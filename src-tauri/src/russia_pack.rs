use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use serde_json::json;
use log::{info, warn, error};

const INSIDE_URL: &str = "https://raw.githubusercontent.com/itdoginfo/allow-domains/main/Russia/inside-raw.lst";
const OUTSIDE_URL: &str = "https://raw.githubusercontent.com/itdoginfo/allow-domains/main/Russia/outside-raw.lst";

const INSIDE_BASE: &str = include_str!("inside_base.lst");
const OUTSIDE_BASE: &str = include_str!("outside_base.lst");

pub fn ensure_russia_pack_files(app_data_dir: &Path) {
    // Запускаем в отдельном потоке, чтобы не блокировать UI
    let app_data_dir_clone = app_data_dir.to_path_buf();
    std::thread::spawn(move || {
        update_list(&app_data_dir_clone, "inside.json", INSIDE_URL, INSIDE_BASE);
        update_list(&app_data_dir_clone, "outside.json", OUTSIDE_URL, OUTSIDE_BASE);
        info!("Обновление Russia Pack списков завершено.");
    });
}

fn update_list(dir: &Path, filename: &str, url: &str, base_text: &str) {
    let path = dir.join(filename);
    info!("Скачиваем список {} с {}...", filename, url);
    
    let mut remote_text = String::new();
    match ureq::get(url).timeout(std::time::Duration::from_secs(10)).call() {
        Ok(response) => {
            if let Ok(text) = response.into_string() {
                remote_text = text;
                info!("Успешно скачан список для: {}", filename);
            }
        }
        Err(e) => {
            warn!("Не удалось скачать {}: {}", filename, e);
        }
    }

    if let Err(e) = save_merged_rule_set(base_text, &remote_text, &path) {
        error!("Ошибка сохранения {}: {}", filename, e);
    } else {
        info!("Успешно обновлён: {}", filename);
    }
}

fn save_merged_rule_set(base_text: &str, remote_text: &str, path: &PathBuf) -> Result<(), String> {
    let mut domains_set = HashSet::new();

    for line in base_text.lines().chain(remote_text.lines()) {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            if !trimmed.ends_with(".ua") {
                domains_set.insert(trimmed.to_string());
            }
        }
    }

    let mut domains: Vec<String> = domains_set.into_iter().collect();
    domains.sort(); // Сортируем для порядка

    let rule_set = json!({
        "version": 1,
        "rules": [
            {
                "domain_suffix": domains
            }
        ]
    });

    let json_str = serde_json::to_string(&rule_set).map_err(|e| format!("json error: {}", e))?;
    fs::write(path, json_str).map_err(|e| format!("io error: {}", e))?;
    Ok(())
}
