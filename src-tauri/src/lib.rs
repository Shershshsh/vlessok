// ============================================================
// lib.rs — Основная библиотека бэкенда vlessok
// ============================================================
// Здесь:
//   - Подключаем модули (config, singbox)
//   - Определяем Tauri-команды для вызова из JavaScript
//   - Инициализируем SingBoxManager как глобальное состояние Tauri
// ============================================================

// Подключаем наши модули
mod config;   // Парсер VLESS URL → sing-box JSON
mod platform; // OS-специфичные утилиты (Job Objects)
mod singbox;  // Управление процессом sing-box

use singbox::SingBoxManager;
use std::sync::Mutex;
use tauri::{Manager, State};

// ============================================================
// Тип для глобального состояния — менеджер sing-box
// Mutex нужен для безопасного доступа из разных команд
// ============================================================
// AppState не нужен pub снаружи модуля — используется только внутри Tauri
struct AppState {
    manager: Mutex<SingBoxManager>,
}

// ============================================================
// Tauri-команды (вызываются из JavaScript через invoke())
// ============================================================

/// Подключиться к VPN: парсит VLESS-URL, запускает sing-box.
/// Вызов из JS: await invoke("connect_vless", { url: "vless://..." })
#[tauri::command]
fn connect_vless(
    url: String,
    mode: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    log::info!("Получена команда connect_vless (режим: {})", mode);

    let conn_mode = if mode == "tun" {
        config::ConnectionMode::Tun
    } else {
        config::ConnectionMode::Mixed
    };

    // Шаг 1: Парсим VLESS-URL и генерируем JSON-конфиг
    let config_json = config::vless_url_to_singbox_config(&url, conn_mode)
        .map_err(|e| format!("Ошибка парсинга URL: {}", e))?;

    log::info!("Конфиг sing-box сгенерирован");

    // Шаг 2: Запускаем sing-box с этим конфигом
    let manager = state.manager.lock()
        .map_err(|e| format!("Внутренняя ошибка (mutex): {}", e))?;

    manager.start(config_json)
        .map_err(|e| format!("Ошибка запуска sing-box: {}", e))?;

    Ok("connected".to_string())
}

/// Отключиться от VPN: останавливает sing-box.
/// Вызов из JS: await invoke("disconnect")
#[tauri::command]
fn disconnect(state: State<'_, AppState>) -> Result<String, String> {
    log::info!("Получена команда disconnect");

    let manager = state.manager.lock()
        .map_err(|e| format!("Внутренняя ошибка (mutex): {}", e))?;

    manager.stop()
        .map_err(|e| format!("Ошибка остановки sing-box: {}", e))?;

    Ok("disconnected".to_string())
}

/// Проверить статус: запущен ли sing-box.
/// Вызов из JS: await invoke("is_connected")
/// Возвращает true/false
#[tauri::command]
fn is_connected(state: State<'_, AppState>) -> bool {
    let manager = match state.manager.lock() {
        Ok(m) => m,
        Err(_) => return false,
    };
    manager.is_running()
}

#[tauri::command]
fn is_admin() -> bool {
    crate::platform::windows::is_elevated()
}

#[tauri::command]
fn relaunch_as_admin() -> Result<(), String> {
    log::info!("Перезапуск с правами администратора...");
    let exe_path = std::env::current_exe().map_err(|e| e.to_string())?;
    
    // Запускаем через PowerShell с запросом UAC
    let status = std::process::Command::new("powershell")
        .arg("-Command")
        .arg(format!("Start-Process '{}' -Verb RunAs", exe_path.display()))
        .status()
        .map_err(|e| format!("Ошибка при вызове PowerShell: {}", e))?;
    
    if status.success() {
        std::process::exit(0);
    }
    Err("Пользователь отменил запрос прав или произошла ошибка".to_string())
}

#[tauri::command]
fn apply_dns_leak_fix() -> Result<(), String> {
    log::info!("Применяю защиту от DNS-leak...");
    let script = r#"
        Set-DnsClientGlobalSetting -SuffixSearchList @()
        reg add "HKLM\SOFTWARE\Policies\Microsoft\Windows NT\DNSClient" /v DisableSmartNameResolution /t REG_DWORD /d 1 /f
        reg add "HKLM\SYSTEM\CurrentControlSet\Services\Dnscache\Parameters" /v DisableParallelAandAAAA /t REG_DWORD /d 1 /f
    "#;
    let status = std::process::Command::new("powershell")
        .arg("-Command")
        .arg(script)
        .status()
        .map_err(|e| format!("Ошибка PowerShell: {}", e))?;
    
    if status.success() {
        Ok(())
    } else {
        Err("Не удалось применить настройки DNS. Нужны права администратора.".to_string())
    }
}

#[tauri::command]
fn reset_network() -> Result<String, String> {
    log::info!("Сброс сетевых настроек...");
    
    // 1. Удаляем TUN-интерфейс
    let _ = std::process::Command::new("netsh")
        .args(["interface", "delete", "name=vlessok-tun"])
        .status();

    // 2. Отменяем изменения DNS и сбрасываем кэш
    let script = r#"
        Set-DnsClientGlobalSetting -ResetServerAddresses
        reg delete "HKLM\SOFTWARE\Policies\Microsoft\Windows NT\DNSClient" /v DisableSmartNameResolution /f
        reg delete "HKLM\SYSTEM\CurrentControlSet\Services\Dnscache\Parameters" /v DisableParallelAandAAAA /f
        ipconfig /flushdns
    "#;

    let _ = std::process::Command::new("powershell")
        .arg("-Command")
        .arg(script)
        .status();

    Ok("Сеть успешно сброшена.".to_string())
}

#[tauri::command]
fn get_current_external_ip() -> Result<String, String> {
    let timeout = std::time::Duration::from_secs(3);
    
    // Основной сервис api.ipify.org
    if let Ok(response) = ureq::get("https://api.ipify.org").timeout(timeout).call() {
        if let Ok(text) = response.into_string() {
            return Ok(text.trim().to_string());
        }
    }
    
    // Резервный сервис ifconfig.me
    if let Ok(response) = ureq::get("https://ifconfig.me").timeout(timeout).call() {
        if let Ok(text) = response.into_string() {
            return Ok(text.trim().to_string());
        }
    }
    
    Err("Не удалось определить IP".to_string())
}

// ============================================================
// Точка запуска приложения
// ============================================================



#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Инициализируем логирование
    // В дев-режиме покажет все логи. Можно настроить через RUST_LOG=debug
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info")
    ).init();

    log::info!("vlessok запускается...");

    // Очистка зависшего TUN-интерфейса при старте
    let _ = std::process::Command::new("netsh")
        .args(["interface", "delete", "name=vlessok-tun"])
        .output(); // Используем output чтобы не выводить в консоль если ошибка (например, интерфейса нет)

    // Создаём глобальное состояние приложения
    let app_state = AppState {
        manager: Mutex::new(SingBoxManager::new()),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // Обработка закрытия окна (Уровень 1 защиты)
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                log::info!("Окно закрывается, останавливаем sing-box...");
                if let Ok(manager) = window.state::<AppState>().inner().manager.lock() {
                    let _ = manager.stop();
                }
            }
        })
        // Регистрируем глобальное состояние
        .manage(app_state)
        // Регистрируем команды
        .invoke_handler(tauri::generate_handler![
            connect_vless,
            disconnect,
            is_connected,
            is_admin,
            relaunch_as_admin,
            apply_dns_leak_fix,
            reset_network,
            get_current_external_ip,
        ])
        .run(tauri::generate_context!())
        .expect("Ошибка при запуске приложения vlessok");
}
