// ============================================================
// config.rs — Парсер VLESS-URL → JSON-конфиг sing-box
// ============================================================
// Принимает строку вида:
//   vless://UUID@SERVER:PORT/?type=tcp&security=reality&pbk=KEY&...#REMARK
// Возвращает готовый JSON для запуска sing-box.
// ============================================================

use serde_json::json;
use url::Url;
use crate::routing::{RoutingMode, RoutingRules};
use std::path::Path;

/// Основная функция: парсит VLESS-URL и генерирует JSON-конфиг для sing-box.
pub fn vless_url_to_singbox_config(url_str: &str, routing_rules: Option<&RoutingRules>, app_data_dir: &Path) -> Result<String, String> {
    // Обрезаем пробелы по краям — пользователь мог случайно скопировать с пробелом
    let raw_url = url_str.trim();

    // Парсим URL через крейт url
    let parsed = Url::parse(raw_url)
        .map_err(|e| format!("Не удалось разобрать URL: {}", e))?;

    // Проверяем схему — должна быть "vless"
    if parsed.scheme() != "vless" {
        return Err(format!(
            "Неверная схема URL: '{}'. Ожидается 'vless://'",
            parsed.scheme()
        ));
    }

    // --- UUID (находится в поле username) ---
    // В vless://UUID@host:port  — UUID идёт до символа @
    let uuid = parsed.username();
    if uuid.is_empty() {
        return Err("UUID не найден в URL (ожидается vless://UUID@host:port)".to_string());
    }
    // Базовая валидация UUID: должен содержать 4 дефиса и иметь длину 36 символов
    validate_uuid(uuid)?;

    // --- Сервер ---
    let server = parsed
        .host_str()
        .ok_or_else(|| "Адрес сервера не найден в URL".to_string())?
        .to_string();

    // --- Порт ---
    let port = parsed
        .port()
        .ok_or_else(|| "Порт не найден в URL (ожидается vless://UUID@host:PORT)".to_string())?;

    // --- Query-параметры (?key=value&key2=value2) ---
    // Собираем все параметры в HashMap для удобного доступа
    let params: std::collections::HashMap<String, String> = parsed
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    // --- SNI (server_name) — обязательный параметр для Reality ---
    let sni = params
        .get("sni")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            "Параметр 'sni' не найден или пустой. \
             Reality требует SNI (например, sni=www.google.com)".to_string()
        })?
        .clone();

    // --- Public Key для Reality ---
    let public_key = params
        .get("pbk")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Параметр 'pbk' (public key) не найден в URL".to_string())?
        .clone();

    // --- Short ID для Reality ---
    // sid может быть пустой строкой — это допустимо
    let short_id = params
        .get("sid")
        .cloned()
        .unwrap_or_default();

    // --- Fingerprint для uTLS (имитация браузера) ---
    // По умолчанию chrome — самый распространённый
    let fingerprint = params
        .get("fp")
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| "chrome".to_string());

    // --- Flow (необязательный) ---
    // Для XTLS-Vision: flow=xtls-rprx-vision
    // Если отсутствует — в JSON не добавляем
    let flow = params
        .get("flow")
        .filter(|s| !s.is_empty())
        .cloned();

    // --- Remark (имя профиля, идёт после # в URL) ---
    let _remark = parsed
        .fragment()
        .unwrap_or("vlessok-profile")
        .to_string();

    // ============================================================
    // Собираем JSON-конфиг sing-box
    // ============================================================

    // Блок TLS + Reality для outbound
    let tls_block = json!({
        "enabled": true,
        "server_name": sni,
        // uTLS — имитирует TLS-рукопожатие реального браузера
        "utls": {
            "enabled": true,
            "fingerprint": fingerprint
        },
        // Reality — параметры для обхода DPI
        "reality": {
            "enabled": true,
            "public_key": public_key,
            "short_id": short_id
        }
    });

    // Outbound (исходящий) для VLESS
    // Если flow задан — добавляем его, иначе не включаем поле
    let mut vless_outbound = json!({
        "type": "vless",
        "tag": "proxy",
        "server": server,
        "server_port": port,
        "uuid": uuid,
        "tls": tls_block
    });

    if let Some(flow_value) = flow {
        vless_outbound["flow"] = json!(flow_value);
    }

    // domain_resolver нужен для DNS-резолвинга в TUN-режиме
    let direct_outbound = json!({ "type": "direct", "tag": "direct", "domain_resolver": "local-dns" });

    // Базовый конфиг (одинаковый для всех режимов)
    let mut config = json!({
        "log": {
            "level": "debug",
            "timestamp": true
        },
        "outbounds": [
            vless_outbound,
            direct_outbound,
            {
                "type": "block",
                "tag": "block"
            }
        ]
    });

    // --- Настройка TUN и DNS ---
    // Строим CIDR из адреса сервера
        // Если server — это домен, sing-box сам его зарезолвит для route_exclude_address
        // Но route_exclude_address принимает только IP/CIDR. Поэтому мы добавляем правило
        // в route.rules через ip_cidr + domain, что покрывает оба случая.
        let server_cidr = if server.contains(':') {
            // IPv6
            format!("{}/128", server)
        } else if server.chars().next().map_or(false, |c| c.is_ascii_digit()) {
            // IPv4 (начинается с цифры)
            format!("{}/32", server)
        } else {
            // Домен — route_exclude_address не подходит, используем route rule с domain
            String::new()
        };
        let is_ip = !server_cidr.is_empty();

        let tun_inbound = if is_ip {
            json!({
                "type": "tun",
                "tag": "tun-in",
                "interface_name": "vlessok-tun",
                "address": ["172.19.0.1/30"],
                "auto_route": true,
                "strict_route": true,
                "stack": "system",
                "endpoint_independent_nat": false,
                "mtu": 1500,
                "route_exclude_address": [server_cidr]
            })
        } else {
            json!({
                "type": "tun",
                "tag": "tun-in",
                "interface_name": "vlessok-tun",
                "address": ["172.19.0.1/30"],
                "auto_route": true,
                "strict_route": true,
                "stack": "system",
                "endpoint_independent_nat": false,
                "mtu": 1500
            })
        };
        config["inbounds"] = json!([tun_inbound]);
        config["dns"] = json!({
            "servers": [
                {
                    "type": "https",
                    "tag": "remote-dns",
                    "server": "1.1.1.1",
                    "domain_resolver": "local-dns",
                    "detour": "proxy"
                },
                {
                    "type": "udp",
                    "tag": "local-dns",
                    "server": "1.1.1.1",
                    "detour": "direct"
                }
            ],
            "rules": [
                { "domain_suffix": [".local"], "server": "local-dns" }
            ],
            "final": "remote-dns",
            "strategy": "ipv4_only"
        });

        // Если сервер задан доменом, его обязательно нужно резолвить локально, иначе chicken-and-egg
        if !is_ip {
            if let Some(rules_array) = config["dns"]["rules"].as_array_mut() {
                rules_array.insert(0, json!({
                    "domain": [server.clone()],
                    "server": "local-dns"
                }));
            }
        }
        // Строим правила route с учётом типа адреса сервера
        let mut route_rules = vec![
            json!({ "action": "sniff", "sniffer": ["http", "tls", "quic"], "timeout": "100ms" }),
            json!({ "port": [53], "action": "hijack-dns" }),
        ];

        // Явный bypass для VLESS-сервера — предотвращает routing loop
        if is_ip {
            // IP-адрес: правило через ip_cidr
            route_rules.push(json!({
                "ip_cidr": [format!("{}/32", server)],
                "outbound": "direct"
            }));
        } else {
            // Домен: правило через domain
            route_rules.push(json!({
                "domain": [server],
                "outbound": "direct"
            }));
        }

        // Приватные адреса тоже идут через direct
        route_rules.push(json!({ "ip_is_private": true, "outbound": "direct" }));

        // Процессы, которые должны идти через VPN (с наивысшим приоритетом)
        let mut proxy_processes = vec![];
        if let Some(rules) = routing_rules {
            if rules.routing_mode == RoutingMode::Rule {
                for p in &rules.processes {
                    proxy_processes.push(p.clone());
                }
                if rules.geo_rules.iter().any(|r| r == "telegram_combo") {
                    proxy_processes.push("telegram.exe".to_string());
                }
                if rules.geo_rules.iter().any(|r| r == "discord_combo") {
                    proxy_processes.push("discord.exe".to_string());
                }
            }
        }
        if !proxy_processes.is_empty() {
            route_rules.push(json!({
                "process_name": proxy_processes,
                "outbound": "proxy"
            }));
        }

        // Внедряем пользовательские правила маршрутизации
        let mut final_route = "proxy";
        
        let inside_path = app_data_dir.join("inside.json");
        let outside_path = app_data_dir.join("outside.json");

        // rule_sets хранит источники внешних списков (SRS или локальные JSON)
        let mut rule_sets = vec![];
        
        // --- 1. Обязательный outside-pack (всегда идёт напрямую) ---
        // Добавляем источник только если файл существует
        if outside_path.exists() {
            let p_str = outside_path.to_string_lossy().replace('\\', "/");
            rule_sets.push(json!({
                "tag": "outside-pack",
                "type": "local",
                "format": "source",
                "path": p_str
            }));
            
            // Правило маршрутизации: outside-pack всегда идёт напрямую
            route_rules.push(json!({
                "rule_set": ["outside-pack"],
                "outbound": "direct"
            }));
        }

        if let Some(rules) = routing_rules {
            if rules.routing_mode == RoutingMode::Rule {
                final_route = "direct"; // По умолчанию прямой доступ, если выбрано "Правило"

                // Домены
                if !rules.domains.is_empty() {
                    route_rules.push(json!({
                        "domain_suffix": rules.domains,
                        "outbound": "proxy"
                    }));
                }

                // GEO правила (собираем rule_sets и тэги)
                if !rules.geo_rules.is_empty() {
                    let mut proxy_tags = vec![];
                    
                    for geo_rule in &rules.geo_rules {
                        if geo_rule == "russia_pack" {
                            if inside_path.exists() {
                                let p_str = inside_path.to_string_lossy().replace('\\', "/");
                                rule_sets.push(json!({
                                    "tag": "inside-pack",
                                    "type": "local",
                                    "format": "source",
                                    "path": p_str
                                }));
                                proxy_tags.push("inside-pack".to_string());
                            }
                            continue;
                        }

                        if geo_rule == "telegram_combo" {
                            rule_sets.push(json!({
                                "tag": "geosite-telegram",
                                "type": "remote",
                                "format": "binary",
                                "url": "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-telegram.srs",
                                "download_detour": "direct"
                            }));
                            proxy_tags.push("geosite-telegram".to_string());

                            rule_sets.push(json!({
                                "tag": "geoip-telegram",
                                "type": "remote",
                                "format": "binary",
                                "url": "https://raw.githubusercontent.com/SagerNet/sing-geoip/rule-set/geoip-telegram.srs",
                                "download_detour": "direct"
                            }));
                            proxy_tags.push("geoip-telegram".to_string());
                            continue;
                        }

                        if geo_rule == "discord_combo" {
                            rule_sets.push(json!({
                                "tag": "geosite-discord",
                                "type": "remote",
                                "format": "binary",
                                "url": "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-discord.srs",
                                "download_detour": "direct"
                            }));
                            proxy_tags.push("geosite-discord".to_string());

                            rule_sets.push(json!({
                                "tag": "geoip-discord",
                                "type": "remote",
                                "format": "binary",
                                "url": "https://raw.githubusercontent.com/SagerNet/sing-geoip/rule-set/geoip-discord.srs",
                                "download_detour": "direct"
                            }));
                            proxy_tags.push("geoip-discord".to_string());
                            continue;
                        }

                        let tag = geo_rule.replace(":", "-");
                        proxy_tags.push(tag.clone());

                        let url = if geo_rule.starts_with("geosite:") {
                            let name = geo_rule.strip_prefix("geosite:").unwrap();
                            format!("https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-{}.srs", name)
                        } else if geo_rule.starts_with("geoip:") {
                            let name = geo_rule.strip_prefix("geoip:").unwrap();
                            format!("https://raw.githubusercontent.com/SagerNet/sing-geoip/rule-set/geoip-{}.srs", name)
                        } else {
                            continue;
                        };

                        rule_sets.push(json!({
                            "tag": tag,
                            "type": "remote",
                            "format": "binary",
                            "url": url,
                            "download_detour": "direct"
                        }));
                    }
                    if !proxy_tags.is_empty() {
                        route_rules.push(json!({
                            "rule_set": proxy_tags,
                            "outbound": "proxy"
                        }));
                    }
                }
            }
        }

        let mut route_obj = json!({
            "auto_detect_interface": true,
            "find_process": true,
            "rules": route_rules,
            "final": final_route
        });

        if !rule_sets.is_empty() {
            route_obj["rule_set"] = json!(rule_sets);
        }

        config["route"] = route_obj;

    // Сериализуем в красиво отформатированный JSON (с отступами)
    serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Ошибка сериализации JSON: {}", e))
}

/// Базовая валидация UUID.
/// UUID должен иметь формат: 8-4-4-4-12 символов, разделённых дефисами.
/// Пример: 550e8400-e29b-41d4-a716-446655440000
fn validate_uuid(uuid: &str) -> Result<(), String> {
    // Длина стандартного UUID с дефисами = 36 символов
    if uuid.len() != 36 {
        return Err(format!(
            "Неверная длина UUID: {} символов (ожидается 36)",
            uuid.len()
        ));
    }

    // Проверяем позиции дефисов: 8, 13, 18, 23
    let dashes_at: Vec<usize> = uuid
        .chars()
        .enumerate()
        .filter(|(_, c)| *c == '-')
        .map(|(i, _)| i)
        .collect();

    if dashes_at != vec![8, 13, 18, 23] {
        return Err(
            "Неверный формат UUID. Ожидается: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx".to_string(),
        );
    }

    // Все остальные символы должны быть hex (0-9, a-f, A-F)
    for (i, c) in uuid.chars().enumerate() {
        if c == '-' {
            continue;
        }
        if !c.is_ascii_hexdigit() {
            return Err(format!(
                "Неверный символ '{}' в UUID на позиции {} (допустимы только 0-9, a-f, A-F)",
                c, i
            ));
        }
    }

    Ok(())
}

// ============================================================
// Тесты (запускаются командой: cargo test)
// ============================================================
#[cfg(test)]
mod tests {
    use super::*;

    // Тестовый VLESS-URL (с фиктивными данными)
    const TEST_URL: &str = "vless://550e8400-e29b-41d4-a716-446655440000@192.168.1.1:443\
        ?type=tcp&encryption=none&security=reality\
        &pbk=testPublicKey123456789012345678901234567890\
        &fp=chrome&sni=www.google.com&sid=abcdef12\
        &spx=%2F&flow=xtls-rprx-vision#MyServer";

    #[test]
    fn test_valid_url_parses_ok() {
        let app_dir = Path::new("");
        let result = vless_url_to_singbox_config(TEST_URL, None, &app_dir);
        assert!(result.is_ok(), "Парсинг валидного URL должен успешно работать");
    }

    #[test]
    fn test_json_contains_server() {
        let app_dir = Path::new("");
        let json_str = vless_url_to_singbox_config(TEST_URL, None, &app_dir).unwrap();
        assert!(json_str.contains("192.168.1.1"), "JSON должен содержать адрес сервера");
    }

    #[test]
    fn test_json_contains_flow() {
        let app_dir = Path::new("");
        let json_str = vless_url_to_singbox_config(TEST_URL, None, &app_dir).unwrap();
        assert!(json_str.contains("xtls-rprx-vision"), "JSON должен содержать flow");
    }

    #[test]
    fn test_missing_sni_returns_error() {
        let app_dir = Path::new("");
        let url_without_sni = "vless://550e8400-e29b-41d4-a716-446655440000@1.2.3.4:443\
            ?security=reality&pbk=key123";
        let result = vless_url_to_singbox_config(url_without_sni, None, &app_dir);
        assert!(result.is_err(), "URL без SNI должен возвращать ошибку");
    }

    #[test]
    fn test_invalid_uuid_returns_error() {
        let app_dir = Path::new("");
        let bad_url = "vless://not-a-uuid@1.2.3.4:443?sni=x.com&pbk=key";
        let result = vless_url_to_singbox_config(bad_url, None, &app_dir);
        assert!(result.is_err(), "URL с невалидным UUID должен возвращать ошибку");
    }
}
