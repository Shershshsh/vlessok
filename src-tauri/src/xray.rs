// ============================================================
// xray.rs — Управление процессом Xray-core
// ============================================================
// Этот модуль отвечает за:
//   - Запись конфига Xray во временный файл
//   - Запуск xray.exe как дочерний процесс для VLESS + XHTTP / SplitHTTP
//   - Корректную остановку процесса (kill + wait)
//   - Гарантированный kill при уничтожении менеджера (Drop / Job Object)
// ============================================================

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use log::{error, info, warn};
use crate::platform::windows::ProcessJob;

/// Хранит состояние запущенного процесса Xray.
pub struct XrayManager {
    process: Arc<Mutex<Option<Child>>>,
    config_path: Arc<Mutex<Option<PathBuf>>>,
    job: Arc<Mutex<Option<ProcessJob>>>,
    last_stderr: Arc<Mutex<std::collections::VecDeque<String>>>,
}

impl XrayManager {
    /// Создаёт новый менеджер и инициализирует Job Object
    pub fn new() -> Self {
        let job = ProcessJob::new().map_err(|e| {
            warn!("Не удалось создать Job Object для Xray: {}", e);
        }).ok();

        XrayManager {
            process: Arc::new(Mutex::new(None)),
            config_path: Arc::new(Mutex::new(None)),
            job: Arc::new(Mutex::new(job)),
            last_stderr: Arc::new(Mutex::new(std::collections::VecDeque::with_capacity(10))),
        }
    }

    /// Ищет бинарник xray.exe относительно исполняемого файла Tauri-приложения.
    fn resolve_binary_path() -> Result<PathBuf, String> {
        let exe_path = std::env::current_exe()
            .map_err(|e| format!("Не удалось получить путь к приложению: {}", e))?;
        
        let exe_dir = exe_path.parent()
            .ok_or("Не удалось получить директорию приложения")?;

        let mut checked_paths = Vec::new();

        // 1. Для прод-сборки (рядом с .exe в папке binaries)
        let prod_path = exe_dir.join("binaries").join("xray.exe");
        if prod_path.exists() {
            return Ok(prod_path);
        }
        checked_paths.push(prod_path.display().to_string());

        // 2. Для dev-режима (выходим из target/debug)
        let dev_path = exe_dir.join("..").join("..").join("binaries").join("xray.exe");
        if dev_path.exists() {
            return Ok(dev_path);
        }
        checked_paths.push(dev_path.display().to_string());

        // 3. Для dev-режима (на уровень выше, на всякий случай)
        let alt_dev_path = exe_dir.join("..").join("..").join("..").join("src-tauri").join("binaries").join("xray.exe");
        if alt_dev_path.exists() {
            return Ok(alt_dev_path);
        }
        checked_paths.push(alt_dev_path.display().to_string());

        Err(format!("Бинарник xray не найден. Проверены пути:\n- {}", checked_paths.join("\n- ")))
    }

    /// Запускает xray с переданным JSON-конфигом.
    pub fn start(&self, config_json: String, app: tauri::AppHandle) -> Result<(), String> {
        if self.is_running(None) {
            warn!("xray уже запущен, останавливаем перед повторным запуском");
            self.stop()?;
        }

        let binary_path = Self::resolve_binary_path()?;

        // Создаём временный файл конфига
        let config_file = tempfile::Builder::new()
            .prefix("vlessok-xray-")
            .suffix(".json")
            .tempfile()
            .map_err(|e| format!("Не удалось создать временный файл конфига Xray: {}", e))?;

        // Записываем JSON в файл (без UTF-8 BOM)
        {
            let mut file = config_file.as_file();
            file.write_all(config_json.as_bytes())
                .map_err(|e| format!("Не удалось записать конфиг Xray: {}", e))?;
            file.flush()
                .map_err(|e| format!("Не удалось сохранить конфиг Xray: {}", e))?;
        }

        let (_, config_path_kept) = config_file.keep()
            .map_err(|e| format!("Не удалось сохранить временный файл Xray: {}", e))?;

        info!("Конфиг Xray записан в: {}", config_path_kept.display());
        info!("Запускаем Xray: {}", binary_path.display());

        let mut cmd = Command::new(&binary_path);
        cmd.arg("run")
           .arg("-c")
           .arg(&config_path_kept)
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd.spawn()
            .map_err(|e| format!("Не удалось запустить Xray: {}", e))?;

        // Привязываем к Job Object
        if let Some(job) = self.job.lock().unwrap().as_ref() {
            if let Err(e) = job.assign_process(&child) {
                warn!("Не удалось привязать Xray к Job Object: {}", e);
            } else {
                info!("Процесс Xray успешно привязан к Job Object");
            }
        }

        // Очищаем буфер логов перед новым запуском
        if let Ok(mut buf) = self.last_stderr.lock() {
            buf.clear();
        }

        fn spawn_log_reader<R: std::io::Read + Send + 'static>(
            stream: R,
            stream_name: &'static str,
            app_clone: tauri::AppHandle,
            log_buf: Arc<Mutex<std::collections::VecDeque<String>>>,
        ) {
            thread::spawn(move || {
                use std::io::{BufRead, BufReader};
                use tauri::Emitter;
                let reader = BufReader::new(stream);
                for line in reader.lines() {
                    match line {
                        Ok(l) => {
                            if l.contains("FATAL")
                                || l.contains("fatal")
                                || l.contains("Failed to start")
                                || l.contains("[Error]")
                            {
                                error!("[xray {}] {}", stream_name, l);
                                let _ = app_clone.emit("xray-error", l.clone());
                            } else {
                                info!("[xray {}] {}", stream_name, l);
                            }

                            let mut buf = log_buf.lock().unwrap();
                            if buf.len() >= 10 {
                                buf.pop_front();
                            }
                            buf.push_back(l);
                        }
                        Err(e) => warn!("[xray {}] ошибка чтения: {}", stream_name, e),
                    }
                }
            });
        }

        // Xray пишет основные логи и ошибки запуска в stdout, перехватываем оба потока
        if let Some(stdout) = child.stdout.take() {
            spawn_log_reader(stdout, "stdout", app.clone(), self.last_stderr.clone());
        }

        if let Some(stderr) = child.stderr.take() {
            spawn_log_reader(stderr, "stderr", app.clone(), self.last_stderr.clone());
        }

        info!("xray запущен, PID: {:?}", child.id());

        *self.process.lock().unwrap() = Some(child);
        if let Some(old_path) = self.config_path.lock().unwrap().replace(config_path_kept) {
            if old_path.exists() {
                let _ = std::fs::remove_file(&old_path);
            }
        }

        Ok(())
    }

    /// Возвращает последние строки лога Xray
    pub fn last_logs(&self) -> String {
        self.last_stderr
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Останавливает Xray
    pub fn stop(&self) -> Result<(), String> {
        let mut process_guard = self.process.lock().unwrap();

        if let Some(mut child) = process_guard.take() {
            info!("Останавливаем Xray (PID: {:?})", child.id());
            let _ = child.kill();
            let _ = child.wait();
        }

        let mut config_guard = self.config_path.lock().unwrap();
        if let Some(path) = config_guard.take() {
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
        }

        Ok(())
    }

    /// Проверяет статус Xray
    pub fn is_running(&self, app: Option<&tauri::AppHandle>) -> bool {
        let mut process_guard = self.process.lock().unwrap();

        if let Some(child) = process_guard.as_mut() {
            match child.try_wait() {
                Ok(None) => true,
                Ok(Some(status)) => {
                    error!("Xray завершился со статусом: {}", status);
                    if let Some(a) = app {
                        use tauri::Emitter;
                        let recent = self.last_logs();
                        let msg = if recent.is_empty() {
                            format!("Xray неожиданно завершился. Код: {}", status)
                        } else {
                            format!("Xray неожиданно завершился (код: {}).\nПоследние логи:\n{}", status, recent)
                        };
                        let _ = a.emit("xray-error", msg);
                    }
                    *process_guard = None;
                    false
                }
                Err(e) => {
                    warn!("Ошибка проверки статуса Xray: {}", e);
                    false
                }
            }
        } else {
            false
        }
    }
}

impl Drop for XrayManager {
    fn drop(&mut self) {
        let mut process_guard = self.process.lock().unwrap();
        if let Some(mut child) = process_guard.take() {
            warn!("XrayManager уничтожается, убиваем Xray (PID: {:?})", child.id());
            let _ = child.kill();
            let _ = child.wait();
        }

        let mut config_guard = self.config_path.lock().unwrap();
        if let Some(path) = config_guard.take() {
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}
