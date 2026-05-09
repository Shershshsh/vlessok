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
mod singbox;  // Управление процессом sing-box

use singbox::SingBoxManager;
use std::sync::Mutex;
use tauri::State;

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
    state: State<'_, AppState>,
) -> Result<String, String> {
    log::info!("Получена команда connect_vless");

    // Шаг 1: Парсим VLESS-URL и генерируем JSON-конфиг
    let config_json = config::vless_url_to_singbox_config(&url)
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

    // Создаём глобальное состояние приложения
    let app_state = AppState {
        manager: Mutex::new(SingBoxManager::new()),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // Регистрируем глобальное состояние
        .manage(app_state)
        // Регистрируем команды — все три должны быть здесь
        .invoke_handler(tauri::generate_handler![
            connect_vless,
            disconnect,
            is_connected,
        ])
        .run(tauri::generate_context!())
        .expect("Ошибка при запуске приложения vlessok");
}
