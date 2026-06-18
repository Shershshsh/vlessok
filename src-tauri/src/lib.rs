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
pub mod routing;
pub mod russia_pack;

use singbox::SingBoxManager;
use std::sync::Mutex;
use std::path::PathBuf;
use tauri::{Manager, State};

// ============================================================
// Тип для глобального состояния — менеджер sing-box
// Mutex нужен для безопасного доступа из разных команд
// ============================================================
struct AppState {
    manager: Mutex<SingBoxManager>,
    routing_rules: Mutex<routing::RoutingRules>,
    icon_cache: Mutex<std::collections::HashMap<String, String>>,
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
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    log::info!("Получена команда connect_vless");

    let app_data_dir = app_handle.path().app_data_dir().unwrap_or_default();
    
    // Получаем текущие правила из state
    let rules = {
        let rules_lock = state.routing_rules.lock().unwrap();
        rules_lock.clone()
    };

    // Шаг 1: Парсим VLESS-URL и генерируем JSON-конфиг
    let config_json = config::vless_url_to_singbox_config(&url, Some(&rules), &app_data_dir)
        .map_err(|e| format!("Ошибка парсинга VLESS: {}", e))?;

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
// Команды маршрутизации
// ============================================================

#[tauri::command]
fn get_routing_rules(state: State<'_, AppState>) -> routing::RoutingRules {
    state.routing_rules.lock().unwrap().clone()
}

#[tauri::command]
fn set_routing_mode(mode: String, state: State<'_, AppState>, app_handle: tauri::AppHandle) -> Result<(), String> {
    let mut rules = state.routing_rules.lock().unwrap();
    let routing_mode = if mode == "rule" {
        routing::RoutingMode::Rule
    } else {
        routing::RoutingMode::Global
    };
    rules.set_mode(routing_mode);
    
    let path = app_handle.path().app_data_dir().unwrap_or_default();
    rules.save(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn add_domain_rule(domain: String, state: State<'_, AppState>, app_handle: tauri::AppHandle) -> Result<String, String> {
    let mut rules = state.routing_rules.lock().unwrap();
    let normalized = rules.add_domain(&domain)?;
    
    let path = app_handle.path().app_data_dir().unwrap_or_default();
    rules.save(&path).map_err(|e| e.to_string())?;
    Ok(normalized)
}

#[tauri::command]
fn remove_domain_rule(domain: String, state: State<'_, AppState>, app_handle: tauri::AppHandle) -> Result<(), String> {
    let mut rules = state.routing_rules.lock().unwrap();
    rules.remove_domain(&domain);
    
    let path = app_handle.path().app_data_dir().unwrap_or_default();
    rules.save(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn add_geo_rule(rule: String, state: State<'_, AppState>, app_handle: tauri::AppHandle) -> Result<String, String> {
    let mut rules = state.routing_rules.lock().unwrap();
    let added = rules.add_geo_rule(&rule)?;
    
    let path = app_handle.path().app_data_dir().unwrap_or_default();
    rules.save(&path).map_err(|e| e.to_string())?;
    Ok(added)
}

#[tauri::command]
fn remove_geo_rule(rule: String, state: State<'_, AppState>, app_handle: tauri::AppHandle) -> Result<(), String> {
    let mut rules = state.routing_rules.lock().unwrap();
    rules.remove_geo_rule(&rule);
    
    let path = app_handle.path().app_data_dir().unwrap_or_default();
    rules.save(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn add_process_rule(process: String, state: State<'_, AppState>, app_handle: tauri::AppHandle) -> Result<String, String> {
    let mut rules = state.routing_rules.lock().unwrap();
    let added = rules.add_process(&process)?;
    
    let path = app_handle.path().app_data_dir().unwrap_or_default();
    rules.save(&path).map_err(|e| e.to_string())?;
    Ok(added)
}

#[tauri::command]
fn remove_process_rule(process: String, state: State<'_, AppState>, app_handle: tauri::AppHandle) -> Result<(), String> {
    let mut rules = state.routing_rules.lock().unwrap();
    rules.remove_process(&process);
    
    let path = app_handle.path().app_data_dir().unwrap_or_default();
    rules.save(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_all_routing_rules(new_rules: routing::RoutingRules, state: State<'_, AppState>, app_handle: tauri::AppHandle) -> Result<(), String> {
    let mut rules = state.routing_rules.lock().unwrap();
    *rules = new_rules;

    let path = app_handle.path().app_data_dir().unwrap_or_default();
    rules.save(&path).map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct ProcessInfo {
    name: String,
    is_app: bool,
}

#[tauri::command]
fn get_running_processes() -> Vec<ProcessInfo> {
    use std::collections::HashSet;
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, IsWindowVisible, GetWindowThreadProcessId};

    let mut sys = sysinfo::System::new_all();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let mut app_pids: HashSet<u32> = HashSet::new();

    unsafe extern "system" fn enum_window_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        if IsWindowVisible(hwnd).into() {
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid != 0 {
                let pids = &mut *(lparam.0 as *mut HashSet<u32>);
                pids.insert(pid);
            }
        }
        BOOL::from(true)
    }

    unsafe {
        let _ = EnumWindows(Some(enum_window_callback), LPARAM(&mut app_pids as *mut _ as isize));
    }

    let mut apps = HashSet::new();
    let mut backgrounds = HashSet::new();

    for (pid, process) in sys.processes() {
        if let Some(name) = process.name().to_str() {
            if name.to_lowercase().ends_with(".exe") {
                if app_pids.contains(&pid.as_u32()) {
                    apps.insert(name.to_string());
                } else {
                    backgrounds.insert(name.to_string());
                }
            }
        }
    }
    
    let mut result = Vec::new();
    let mut apps_vec: Vec<_> = apps.into_iter().collect();
    apps_vec.sort_by_key(|a| a.to_lowercase());
    for a in apps_vec {
        result.push(ProcessInfo { name: a, is_app: true });
    }

    let mut bg_vec: Vec<_> = backgrounds.into_iter().collect();
    bg_vec.sort_by_key(|a| a.to_lowercase());
    for b in bg_vec {
        if !result.iter().any(|p| p.name == b) {
            result.push(ProcessInfo { name: b, is_app: false });
        }
    }

    result
}

#[tauri::command]
fn get_process_icons_batched(process_names: Vec<String>, state: State<'_, AppState>) -> std::collections::HashMap<String, String> {
    let mut cache = state.icon_cache.lock().unwrap();
    let mut missing_names = Vec::new();
    let mut result = std::collections::HashMap::new();

    for name in &process_names {
        if let Some(b64) = cache.get(name) {
            result.insert(name.clone(), b64.clone());
        } else {
            missing_names.push(name.clone());
        }
    }

    if missing_names.is_empty() {
        return result;
    }

    use sysinfo::System;
    let mut sys = System::new_all();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let mut paths_to_fetch = Vec::new();
    let mut path_to_name = std::collections::HashMap::new();

    for name in missing_names {
        // Найдём путь к exe для этого имени
        let mut exe_path = String::new();
        for (_, process) in sys.processes() {
            if let Some(pname) = process.name().to_str() {
                if pname.to_lowercase() == name.to_lowercase() {
                    if let Some(path) = process.exe() {
                        exe_path = path.to_string_lossy().to_string();
                        break;
                    }
                }
            }
        }
        if !exe_path.is_empty() {
            paths_to_fetch.push(exe_path.clone());
            path_to_name.insert(exe_path, name);
        } else {
            // Заглушка, чтобы больше не искать
            cache.insert(name, "".to_string());
        }
    }

    if paths_to_fetch.is_empty() {
        return result;
    }

    // Формируем PS-массив путей
    let ps_array = paths_to_fetch
        .iter()
        .map(|p| format!("'{}'", p.replace("'", "''")))
        .collect::<Vec<_>>()
        .join(", ");

    let script = format!(
        r#"
        Add-Type -AssemblyName System.Drawing
        $paths = @({})
        $res = @{{}}
        foreach ($path in $paths) {{
            try {{
                $icon = [System.Drawing.Icon]::ExtractAssociatedIcon($path)
                if ($icon -ne $null) {{
                    $bmp = $icon.ToBitmap()
                    $ms = New-Object System.IO.MemoryStream
                    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
                    $res[$path] = "data:image/png;base64," + [Convert]::ToBase64String($ms.ToArray())
                }}
            }} catch {{}}
        }}
        $res | ConvertTo-Json -Compress
        "#,
        ps_array
    );

    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let output = std::process::Command::new("powershell")
        .creation_flags(CREATE_NO_WINDOW)
        .arg("-NoProfile")
        .arg("-Command")
        .arg(&script)
        .output();

    if let Ok(out) = output {
        let json_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !json_str.is_empty() {
            if let Ok(parsed) = serde_json::from_str::<std::collections::HashMap<String, String>>(&json_str) {
                for (path, b64) in parsed {
                    if let Some(name) = path_to_name.get(&path) {
                        cache.insert(name.clone(), b64.clone());
                        result.insert(name.clone(), b64);
                    }
                }
            }
        }
    }

    // Для тех, кого PowerShell не смог распарсить, ставим пустую строку в кэш
    for (path, name) in path_to_name {
        if !cache.contains_key(&name) {
            cache.insert(name, "".to_string());
        }
    }

    result
}

// ============================================================
// Точка запуска приложения
// ============================================================



#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Инициализируем логирование
    // В дев-режиме покажет все логи. Можно настроить через RUST_LOG=debug
    if let Ok(log_file) = std::fs::File::create("vlessok_debug.log") {
        env_logger::Builder::from_env(
            env_logger::Env::default().default_filter_or("debug")
        )
        .target(env_logger::Target::Pipe(Box::new(log_file)))
        .init();
    } else {
        env_logger::Builder::from_env(
            env_logger::Env::default().default_filter_or("debug")
        ).init();
    }

    log::info!("vlessok запускается...");

    // Очистка зависшего TUN-интерфейса при старте
    let _ = std::process::Command::new("netsh")
        .args(["interface", "delete", "name=vlessok-tun"])
        .output(); // Используем output чтобы не выводить в консоль если ошибка (например, интерфейса нет)

    // Создаём глобальное состояние приложения
    let app_state = AppState {
        manager: Mutex::new(SingBoxManager::new()),
        routing_rules: Mutex::new(routing::RoutingRules::default()),
        icon_cache: Mutex::new(std::collections::HashMap::new()),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let path = app.path().app_data_dir().unwrap_or_default();
            
            // Пытаемся загрузить правила при старте
            if let Ok(rules) = routing::RoutingRules::load(&path) {
                let state: State<AppState> = app.state();
                *state.routing_rules.lock().unwrap() = rules;
            }

            // Запускаем фоновое обновление списков Russia Pack
            russia_pack::ensure_russia_pack_files(&path);

            Ok(())
        })
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
            get_routing_rules,
            set_routing_mode,
            add_domain_rule,
            remove_domain_rule,
            add_geo_rule,
            remove_geo_rule,
            add_process_rule,
            remove_process_rule,
            set_all_routing_rules,
            get_running_processes,
            get_process_icons_batched
        ])
        .run(tauri::generate_context!())
        .expect("Ошибка при запуске приложения vlessok");
}
