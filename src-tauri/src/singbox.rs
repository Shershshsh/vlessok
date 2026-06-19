// ============================================================
// singbox.rs — Управление процессом sing-box
// ============================================================
// Этот модуль отвечает за:
//   - Запись конфига sing-box во временный файл
//   - Запуск sing-box.exe как дочерний процесс
//   - Корректную остановку процесса (kill + wait)
//   - Гарантированный kill при уничтожении менеджера (Drop)
// ============================================================

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use log::{error, info, warn};
use crate::platform::windows::ProcessJob;

/// Хранит состояние запущенного sing-box процесса.
/// Arc<Mutex<...>> позволяет безопасно передавать между потоками.
pub struct SingBoxManager {
    /// Handle на запущенный процесс (None если не запущен)
    process: Arc<Mutex<Option<Child>>>,
    /// Путь к временному файлу конфига (нужен чтобы удалить после остановки)
    config_path: Arc<Mutex<Option<PathBuf>>>,
    /// Job Object для гарантии убиения дочернего процесса при смерти vlessok (для Windows)
    job: Arc<Mutex<Option<ProcessJob>>>,
    /// Буфер для хранения последних строк логов (для вывода при краше)
    last_stderr: Arc<Mutex<std::collections::VecDeque<String>>>,
}

impl SingBoxManager {
    /// Создаёт новый менеджер и инициализирует глобальный Job Object
    pub fn new() -> Self {
        // Создаем Job Object (будет создан только один раз для менеджера)
        let job = ProcessJob::new().map_err(|e| {
            warn!("Не удалось создать Job Object: {}", e);
        }).ok();

        SingBoxManager {
            process: Arc::new(Mutex::new(None)),
            config_path: Arc::new(Mutex::new(None)),
            job: Arc::new(Mutex::new(job)),
            last_stderr: Arc::new(Mutex::new(std::collections::VecDeque::with_capacity(10))),
        }
    }

    /// Ищет бинарник sing-box относительно исполняемого файла Tauri-приложения.
    fn resolve_binary_path() -> Result<PathBuf, String> {
        let exe_path = std::env::current_exe()
            .map_err(|e| format!("Не удалось получить путь к приложению: {}", e))?;
        
        let exe_dir = exe_path.parent()
            .ok_or("Не удалось получить директорию приложения")?;

        let mut checked_paths = Vec::new();

        // 1. Для прод-сборки (рядом с .exe в папке binaries)
        let prod_path = exe_dir.join("binaries").join("sing-box.exe");
        if prod_path.exists() {
            return Ok(prod_path);
        }
        checked_paths.push(prod_path.display().to_string());

        // 2. Для dev-режима (выходим из target/debug)
        let dev_path = exe_dir.join("..").join("..").join("binaries").join("sing-box.exe");
        if dev_path.exists() {
            return Ok(dev_path);
        }
        checked_paths.push(dev_path.display().to_string());

        // 3. Для dev-режима (на уровень выше, на всякий случай)
        let alt_dev_path = exe_dir.join("..").join("..").join("..").join("src-tauri").join("binaries").join("sing-box.exe");
        if alt_dev_path.exists() {
            return Ok(alt_dev_path);
        }
        checked_paths.push(alt_dev_path.display().to_string());

        Err(format!("Бинарник sing-box не найден. Проверены пути:\n- {}", checked_paths.join("\n- ")))
    }

    /// Запускает sing-box с переданным JSON-конфигом.
    /// Если уже запущен — сначала останавливает старый процесс.
    pub fn start(&self, config_json: String, app: tauri::AppHandle) -> Result<(), String> {
        // Если процесс уже запущен — остановить
        if self.is_running(None) {
            warn!("sing-box уже запущен, останавливаем перед повторным запуском");
            self.stop()?;
        }

        // Резолвим путь к бинарнику
        let binary_path = Self::resolve_binary_path()?;

        // Проверяем наличие wintun.dll рядом с sing-box.exe (требуется для TUN)
        if let Some(exe_dir) = binary_path.parent() {
            let wintun_path = exe_dir.join("wintun.dll");
            if !wintun_path.exists() {
                return Err(format!(
                    "Файл wintun.dll не найден по пути: {}. Он необходим для работы VPN. Пожалуйста, переустановите приложение.",
                    wintun_path.display()
                ));
            }
        }

        // Записываем конфиг во временный файл
        // tempfile создаёт файл в системной TEMP-директории
        let config_file = tempfile::Builder::new()
            .prefix("vlessok-")
            .suffix(".json")
            .tempfile()
            .map_err(|e| format!("Не удалось создать временный файл конфига: {}", e))?;

        // Запоминаем путь на случай ошибки при keep()
        let _config_file_path = config_file.path().to_path_buf();

        // Записываем JSON в файл
        {
            let mut file = config_file.as_file();
            file.write_all(config_json.as_bytes())
                .map_err(|e| format!("Не удалось записать конфиг: {}", e))?;
            file.flush()
                .map_err(|e| format!("Не удалось сохранить конфиг: {}", e))?;
        }

        // persist() — не удаляет файл при выходе из области видимости
        // Нам нужен постоянный файл пока работает sing-box
        let (_, config_path_kept) = config_file.keep()
            .map_err(|e| format!("Не удалось сохранить временный файл: {}", e))?;

        info!("Конфиг записан в: {}", config_path_kept.display());
        info!("Запускаем sing-box: {}", binary_path.display());

        // Строим команду запуска sing-box
        let mut cmd = Command::new(&binary_path);
        cmd.arg("run")
           .arg("-c")
           .arg(&config_path_kept)
           // Перехватываем stdout и stderr для логирования
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        // На Windows: скрываем консольное окно sing-box
        // CREATE_NO_WINDOW = 0x08000000
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        // Запускаем процесс
        let mut child = cmd.spawn()
            .map_err(|e| format!("Не удалось запустить sing-box: {}", e))?;

        // Привязываем к Job Object СРАЗУ ПОСЛЕ ЗАПУСКА (Уровень 2 защиты)
        if let Some(job) = self.job.lock().unwrap().as_ref() {
            if let Err(e) = job.assign_process(&child) {
                warn!("Не удалось привязать sing-box к Job Object: {}", e);
            } else {
                info!("Процесс sing-box успешно привязан к Job Object");
            }
        }

        // Перехватываем stdout и stderr в фоновые потоки для логирования
        if let Some(stdout) = child.stdout.take() {
            thread::spawn(move || {
                use std::io::{BufRead, BufReader};
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    match line {
                        Ok(l) => info!("[sing-box stdout] {}", l),
                        Err(e) => warn!("[sing-box stdout] ошибка чтения: {}", e),
                    }
                }
            });
        }

        if let Some(stderr) = child.stderr.take() {
            let app_clone = app.clone();
            let stderr_buf = self.last_stderr.clone();
            thread::spawn(move || {
                use std::io::{BufRead, BufReader};
                use tauri::Emitter;
                let reader = BufReader::new(stderr);
                for line in reader.lines() {
                    match line {
                        Ok(l) => {
                            // sing-box пишет в stderr нормальные логи, не только ошибки
                            if l.contains("FATAL") || l.contains("fatal") || l.contains("ERROR") || l.contains("error") {
                                error!("[sing-box] {}", l);
                                let _ = app_clone.emit("singbox-error", l.clone());
                            } else {
                                info!("[sing-box] {}", l);
                            }
                            
                            // Сохраняем в буфер последних строк
                            let mut buf = stderr_buf.lock().unwrap();
                            if buf.len() >= 10 {
                                buf.pop_front();
                            }
                            buf.push_back(l);
                        }
                        Err(e) => warn!("[sing-box stderr] ошибка чтения: {}", e),
                    }
                }
            });
        }

        info!("sing-box запущен, PID: {:?}", child.id());

        // Сохраняем handle процесса и путь к конфигу
        *self.process.lock().unwrap() = Some(child);
        *self.config_path.lock().unwrap() = Some(config_path_kept);

        Ok(())
    }

    /// Останавливает sing-box.
    /// Убивает процесс и удаляет временный файл конфига.
    pub fn stop(&self) -> Result<(), String> {
        let mut process_guard = self.process.lock().unwrap();

        if let Some(mut child) = process_guard.take() {
            info!("Останавливаем sing-box (PID: {:?})", child.id());

            // Убиваем процесс
            child.kill()
                .map_err(|e| format!("Не удалось убить процесс sing-box: {}", e))?;

            // Ждём завершения чтобы не создавать зомби-процесс
            match child.wait() {
                Ok(status) => info!("sing-box завершился со статусом: {}", status),
                Err(e) => warn!("Ошибка ожидания завершения sing-box: {}", e),
            }
        } else {
            info!("sing-box не был запущен, нечего останавливать");
        }

        // Удаляем временный файл конфига
        let mut config_guard = self.config_path.lock().unwrap();
        if let Some(path) = config_guard.take() {
            if path.exists() {
                if let Err(e) = std::fs::remove_file(&path) {
                    warn!("Не удалось удалить временный файл конфига {}: {}", path.display(), e);
                } else {
                    info!("Временный файл конфига удалён: {}", path.display());
                }
            }
        }

        Ok(())
    }

    /// Проверяет, запущен ли sing-box в данный момент.
    /// Также обнаруживает если процесс завершился сам по себе (краш).
    pub fn is_running(&self, app: Option<&tauri::AppHandle>) -> bool {
        let mut process_guard = self.process.lock().unwrap();

        if let Some(child) = process_guard.as_mut() {
            // try_wait() — неблокирующая проверка статуса процесса
            match child.try_wait() {
                Ok(None) => {
                    // Процесс ещё работает
                    true
                }
                Ok(Some(status)) => {
                    // Процесс завершился (возможно, краш)
                    error!("sing-box неожиданно завершился со статусом: {}", status);
                    if let Some(a) = app {
                        use tauri::Emitter;
                        let mut recent_logs = String::new();
                        {
                            let buf = self.last_stderr.lock().unwrap();
                            if !buf.is_empty() {
                                recent_logs = format!("\nПоследние логи:\n{}", buf.iter().cloned().collect::<Vec<_>>().join("\n"));
                            }
                        }
                        
                        let msg = format!("sing-box неожиданно завершился (crash). Код: {}\nВозможно, конфликт с другим VPN (порт уже занят) или ошибка в конфигурации. Закройте другие VPN-приложения и попробуйте снова.{}", status, recent_logs);
                        let _ = a.emit("singbox-error", msg);
                    }
                    // Очищаем handle
                    *process_guard = None;
                    false
                }
                Err(e) => {
                    warn!("Ошибка проверки статуса sing-box: {}", e);
                    false
                }
            }
        } else {
            false
        }
    }
}

/// При уничтожении менеджера — гарантированно убиваем sing-box.
/// Это важно: если приложение падает, sing-box не должен оставаться зомби.
impl Drop for SingBoxManager {
    fn drop(&mut self) {
        let mut process_guard = self.process.lock().unwrap();
        if let Some(mut child) = process_guard.take() {
            warn!("SingBoxManager уничтожается, убиваем sing-box (PID: {:?})", child.id());
            let _ = child.kill();
            let _ = child.wait();
        }

        // Удаляем файл конфига если он ещё есть
        let mut config_guard = self.config_path.lock().unwrap();
        if let Some(path) = config_guard.take() {
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}
