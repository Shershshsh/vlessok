// ============================================================
// config.rs — Парсер URL (VLESS, Hysteria 1/2, XHTTP) → Конфиги
// ============================================================
// Поддерживаемые схемы:
//   - vless://... (TCP, Reality, Vision, WS, gRPC — sing-box)
//   - vless://... (type=xhttp / splithttp — связка Xray + sing-box)
//   - hysteria2://... / hy2://... (sing-box)
//   - hysteria://... (sing-box)
// ============================================================

use serde_json::json;
use url::Url;
use crate::routing::{RoutingMode, RoutingRules};
use std::collections::HashMap;
use std::path::Path;

/// Результат парсинга конфигурации
#[derive(Debug, Clone, PartialEq)]
pub enum ProxyConfig {
    /// Запускается только sing-box
    SingBoxOnly(String),
    /// Запускается связка: Xray (для XHTTP) + sing-box (для TUN и маршрутизации)
    XrayChained {
        xray_config: String,
        singbox_config: String,
    },
}

/// Декодирование percent-encoding (URL-decode) без внешних зависимостей
fn percent_decode(input: &str) -> String {
    let mut bytes = Vec::new();
    let mut chars = input.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let h1 = chars.next();
            let h2 = chars.next();
            if let (Some(c1), Some(c2)) = (h1, h2) {
                let hex_str = [c1, c2];
                if let Ok(s) = std::str::from_utf8(&hex_str) {
                    if let Ok(byte) = u8::from_str_radix(s, 16) {
                        bytes.push(byte);
                        continue;
                    }
                }
            }
        }
        bytes.push(b);
    }
    String::from_utf8_lossy(&bytes).to_string()
}

/// Разбирает строку портов вида "20000-30000" или "443,20000-30000" в формат sing-box ("20000:30000", "443:443").
/// Возвращает (первый_одиночный_порт, список_диапазонов).
fn parse_port_ranges(spec: &str) -> (Option<String>, Vec<String>) {
    let mut first_port: Option<String> = None;
    let mut ranges = Vec::new();

    for raw_part in spec.split(',') {
        let part = raw_part.trim();
        if part.is_empty() {
            continue;
        }
        let sep = if part.contains('-') {
            Some('-')
        } else if part.contains(':') {
            Some(':')
        } else {
            None
        };
        if let Some(s) = sep {
            if let Some((a, b)) = part.split_once(s) {
                let a = a.trim();
                let b = b.trim();
                if a.parse::<u16>().is_ok() && b.parse::<u16>().is_ok() {
                    if first_port.is_none() {
                        first_port = Some(a.to_string());
                    }
                    ranges.push(format!("{}:{}", a, b));
                }
            }
        } else if part.parse::<u16>().is_ok() {
            if first_port.is_none() {
                first_port = Some(part.to_string());
            }
            ranges.push(format!("{}:{}", part, part));
        }
    }

    (first_port, ranges)
}

/// Нормализует URL с Hysteria 2 port hopping (например, `hy2://pass@host:20000-30000?...`),
/// чтобы `Url::parse` не падал на диапазоне портов. Возвращает (нормализованный_URL, диапазоны_singbox).
pub fn normalize_port_hopping_url(raw_url: &str) -> (String, Vec<String>) {
    let Some(scheme_pos) = raw_url.find("://") else {
        return (raw_url.to_string(), Vec::new());
    };
    let after_scheme_idx = scheme_pos + 3;
    let after_scheme = &raw_url[after_scheme_idx..];

    let auth_len = after_scheme
        .find(|c| c == '/' || c == '?' || c == '#')
        .unwrap_or(after_scheme.len());
    let authority = &after_scheme[..auth_len];

    let host_offset = authority.rfind('@').map(|i| i + 1).unwrap_or(0);
    let host_port = &authority[host_offset..];

    let colon_in_host_port = if host_port.starts_with('[') {
        host_port.find("]:").map(|i| i + 1)
    } else {
        host_port.find(':')
    };

    let Some(colon_rel) = colon_in_host_port else {
        return (raw_url.to_string(), Vec::new());
    };

    let port_start = after_scheme_idx + host_offset + colon_rel + 1;
    let port_end = after_scheme_idx + auth_len;
    let port_part = &raw_url[port_start..port_end];

    if !port_part.contains('-') && !port_part.contains(',') {
        return (raw_url.to_string(), Vec::new());
    }

    let (first_port, ranges) = parse_port_ranges(port_part);
    if let (Some(first), false) = (first_port, ranges.is_empty()) {
        let mut normalized = String::with_capacity(raw_url.len());
        normalized.push_str(&raw_url[..port_start]);
        normalized.push_str(&first);
        normalized.push_str(&raw_url[port_end..]);
        (normalized, ranges)
    } else {
        (raw_url.to_string(), Vec::new())
    }
}

fn is_private_or_loopback(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback() || ((v6.segments()[0] & 0xfe00) == 0xfc00)
            }
        };
    }
    false
}

fn parse_alpn(params: &HashMap<String, String>) -> Option<Vec<String>> {
    let raw = params.get("alpn")?;
    let list: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if list.is_empty() { None } else { Some(list) }
}

/// Основная точка входа: парсит URL любого поддерживаемого протокола и генерирует конфиги.
pub fn parse_proxy_url(
    url_str: &str,
    routing_rules: Option<&RoutingRules>,
    app_data_dir: &Path,
    socks_port: u16,
) -> Result<ProxyConfig, String> {
    let raw_url = url_str.trim();
    let (normalized_url, port_ranges) = normalize_port_hopping_url(raw_url);

    let parsed = Url::parse(&normalized_url)
        .map_err(|e| format!("Не удалось разобрать URL: {}", e))?;

    let scheme = parsed.scheme().to_lowercase();

    match scheme.as_str() {
        "vless" => parse_vless(&parsed, routing_rules, app_data_dir, socks_port),
        "hysteria2" | "hy2" => parse_hysteria2(&parsed, &port_ranges, routing_rules, app_data_dir),
        "hysteria" => parse_hysteria1(&parsed, routing_rules, app_data_dir),
        other => Err(format!(
            "Неподдерживаемая схема URL: '{}'. Поддерживаются: 'vless://', 'hysteria2://', 'hy2://', 'hysteria://'",
            other
        )),
    }
}

/// Для обратной совместимости со старыми вызовами и тестами
#[allow(dead_code)]
pub fn vless_url_to_singbox_config(
    url_str: &str,
    routing_rules: Option<&RoutingRules>,
    app_data_dir: &Path,
) -> Result<String, String> {
    match parse_proxy_url(url_str, routing_rules, app_data_dir, 10808)? {
        ProxyConfig::SingBoxOnly(cfg) => Ok(cfg),
        ProxyConfig::XrayChained { singbox_config, .. } => Ok(singbox_config),
    }
}

/// Парсинг VLESS URL (с детектом обычного VLESS или XHTTP)
fn parse_vless(
    parsed: &Url,
    routing_rules: Option<&RoutingRules>,
    app_data_dir: &Path,
    socks_port: u16,
) -> Result<ProxyConfig, String> {
    let uuid = parsed.username();
    if uuid.is_empty() {
        return Err("UUID не найден в URL (ожидается vless://UUID@host:port)".to_string());
    }
    validate_uuid(uuid)?;

    let raw_server = parsed
        .host_str()
        .ok_or_else(|| "Адрес сервера не найден в URL".to_string())?;
    let server = raw_server.trim_start_matches('[').trim_end_matches(']').to_string();

    let port = parsed
        .port()
        .ok_or_else(|| "Порт не найден в URL (ожидается vless://UUID@host:PORT)".to_string())?;

    let params: HashMap<String, String> = parsed
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    let net_type = params.get("type").map(|s| s.to_lowercase()).unwrap_or_default();

    // Проверяем, является ли это XHTTP / SplitHTTP
    if net_type == "xhttp" || net_type == "splithttp" {
        // --- VLESS + XHTTP (через Xray-core) ---
        let xray_config = build_xray_xhttp_config(uuid, &server, port, &params, socks_port)?;

        // В sing-box настраиваем socks5 outbound к локальному Xray
        let socks_outbound = json!({
            "type": "socks",
            "tag": "proxy",
            "server": "127.0.0.1",
            "server_port": socks_port
        });

        let singbox_config = build_singbox_config(
            socks_outbound,
            &server,
            routing_rules,
            app_data_dir,
            true, // is_xray_chained
        )?;

        Ok(ProxyConfig::XrayChained {
            xray_config,
            singbox_config,
        })
    } else {
        // --- Обычный VLESS (нативно в sing-box) ---
        let sni = params.get("sni").filter(|s| !s.is_empty()).cloned().unwrap_or_default();
        let public_key = params.get("pbk").filter(|s| !s.is_empty()).cloned().unwrap_or_default();
        let short_id = params.get("sid").cloned().unwrap_or_default();
        let fingerprint = params
            .get("fp")
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| "chrome".to_string());
        let flow = params.get("flow").filter(|s| !s.is_empty()).cloned();

        let security = params.get("security").map(|s| s.as_str()).unwrap_or("none");
        let tls_block = if security == "none" || security.is_empty() {
            json!({ "enabled": false })
        } else {
            let mut t = json!({
                "enabled": true,
                "server_name": sni,
                "utls": {
                    "enabled": true,
                    "fingerprint": fingerprint
                }
            });
            if security == "reality" {
                t["reality"] = json!({
                    "enabled": true,
                    "public_key": public_key,
                    "short_id": short_id
                });
            } else {
                let insecure = params
                    .get("insecure")
                    .or_else(|| params.get("allowInsecure"))
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
                if insecure {
                    t["insecure"] = json!(true);
                }
            }
            if let Some(alpn) = parse_alpn(&params) {
                t["alpn"] = json!(alpn);
            }
            t
        };

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

        // Поддержка WebSocket / gRPC транспортов в sing-box
        if net_type == "ws" {
            let path = params.get("path").cloned().unwrap_or_else(|| "/".to_string());
            let host_header = params.get("host").cloned().unwrap_or_default();
            let mut ws_obj = json!({
                "type": "ws",
                "path": path
            });
            if !host_header.is_empty() {
                ws_obj["headers"] = json!({ "Host": host_header });
            }
            vless_outbound["transport"] = ws_obj;
        } else if net_type == "grpc" {
            let service_name = params.get("serviceName").or_else(|| params.get("path")).cloned().unwrap_or_default();
            vless_outbound["transport"] = json!({
                "type": "grpc",
                "service_name": service_name
            });
        }

        let singbox_config = build_singbox_config(
            vless_outbound,
            &server,
            routing_rules,
            app_data_dir,
            false,
        )?;

        Ok(ProxyConfig::SingBoxOnly(singbox_config))
    }
}

/// Генерация конфига для Xray-core (VLESS + XHTTP / SplitHTTP)
fn build_xray_xhttp_config(
    uuid: &str,
    server: &str,
    port: u16,
    params: &HashMap<String, String>,
    socks_port: u16,
) -> Result<String, String> {
    let security = params.get("security").map(|s| s.as_str()).unwrap_or("none");
    let sni = params.get("sni").filter(|s| !s.is_empty()).cloned().unwrap_or_default();
    let fingerprint = params
        .get("fp")
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| "chrome".to_string());

    if (security == "none" || security.is_empty()) && !is_private_or_loopback(server) {
        return Err("XHTTP без TLS или Reality запрещён в Xray для публичных серверов. Укажите security=tls или security=reality.".to_string());
    }

    let path = params.get("path").cloned().unwrap_or_else(|| "/".to_string());
    let mode = params.get("mode").cloned().unwrap_or_else(|| "auto".to_string());
    let host_header = params.get("host").cloned().unwrap_or_else(|| sni.clone());

    let mut xhttp_settings = json!({
        "path": path,
        "host": host_header,
        "mode": mode
    });

    if let Some(extra_str) = params.get("extra").filter(|s| !s.is_empty()) {
        let extra_val: serde_json::Value = serde_json::from_str(extra_str)
            .map_err(|e| format!("Некорректный JSON в параметре extra: {}", e))?;
        if !extra_val.is_object() {
            return Err("Некорректный JSON в параметре extra: ожидается JSON-объект".to_string());
        }
        xhttp_settings["extra"] = extra_val;
    }

    let mut stream_settings = json!({
        "network": "xhttp",
        "security": security,
        "xhttpSettings": xhttp_settings
    });

    if security == "reality" {
        let pbk = params.get("pbk").cloned().unwrap_or_default();
        let sid = params.get("sid").cloned().unwrap_or_default();
        let spx = params.get("spx").cloned().unwrap_or_default();

        let mut reality_obj = json!({
            "show": false,
            "publicKey": pbk,
            "shortId": sid,
            "serverName": sni,
            "fingerprint": fingerprint
        });
        if !spx.is_empty() {
            reality_obj["spiderX"] = json!(spx);
        }
        stream_settings["realitySettings"] = reality_obj;
    } else if security == "tls" {
        let insecure = params
            .get("insecure")
            .or_else(|| params.get("allowInsecure"))
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let pin = params
            .get("pcs")
            .or_else(|| params.get("pinSHA256"))
            .filter(|p| !p.is_empty());

        if insecure && pin.is_none() {
            return Err("XHTTP: Xray 26+ больше не поддерживает insecure=1 для TLS. Укажите pcs=<SHA256 сертификата в hex> или используйте валидный сертификат.".to_string());
        }

        let mut tls_settings = json!({
            "serverName": sni,
            "fingerprint": fingerprint
        });
        if let Some(p) = pin {
            tls_settings["pinnedPeerCertSha256"] = json!(p);
        }
        if let Some(alpn) = parse_alpn(params) {
            tls_settings["alpn"] = json!(alpn);
        }
        stream_settings["tlsSettings"] = tls_settings;
    }

    let xray_json = json!({
        "log": {
            "loglevel": "warning"
        },
        "inbounds": [
            {
                "tag": "socks-in",
                "port": socks_port,
                "listen": "127.0.0.1",
                "protocol": "socks",
                "settings": {
                    "auth": "noauth",
                    "udp": true
                }
            }
        ],
        "outbounds": [
            {
                "tag": "proxy",
                "protocol": "vless",
                "settings": {
                    "vnext": [
                        {
                            "address": server,
                            "port": port,
                            "users": [
                                {
                                    "id": uuid,
                                    "encryption": "none"
                                }
                            ]
                        }
                    ]
                },
                "streamSettings": stream_settings
            }
        ]
    });

    serde_json::to_string_pretty(&xray_json)
        .map_err(|e| format!("Ошибка сериализации конфига Xray: {}", e))
}

/// Парсинг Hysteria 2 (hysteria2:// или hy2://)
fn parse_hysteria2(
    parsed: &Url,
    url_port_ranges: &[String],
    routing_rules: Option<&RoutingRules>,
    app_data_dir: &Path,
) -> Result<ProxyConfig, String> {
    let raw_server = parsed
        .host_str()
        .ok_or_else(|| "Адрес сервера не найден в URL".to_string())?;
    let server = raw_server.trim_start_matches('[').trim_end_matches(']').to_string();

    let port = parsed.port().unwrap_or(443);

    let params: HashMap<String, String> = parsed
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    // Пароль может быть в parsed.password(), parsed.username() или query-параметрах
    let password = if let Some(pass) = parsed.password() {
        if !pass.is_empty() {
            percent_decode(pass)
        } else {
            percent_decode(parsed.username())
        }
    } else if !parsed.username().is_empty() {
        percent_decode(parsed.username())
    } else if let Some(pass) = params.get("password").or_else(|| params.get("auth")) {
        pass.clone()
    } else {
        return Err("Пароль не найден в URL Hysteria 2 (ожидается hy2://password@host:port)".to_string());
    };

    let sni = params
        .get("sni")
        .or_else(|| params.get("peer"))
        .cloned()
        .unwrap_or_else(|| server.clone());

    let insecure = params
        .get("insecure")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
        || params
            .get("allowInsecure")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

    if let Some(pin) = params.get("pinSHA256").or_else(|| params.get("pin_sha256")) {
        if !pin.is_empty() {
            log::warn!("Параметр pinSHA256 в Hysteria 2 не поддерживается sing-box и будет проигнорирован");
        }
    }

    let mut tls_obj = json!({
        "enabled": true,
        "server_name": sni,
        "insecure": insecure
    });
    if let Some(alpn) = parse_alpn(&params) {
        tls_obj["alpn"] = json!(alpn);
    }

    let mut hy2_outbound = json!({
        "type": "hysteria2",
        "tag": "proxy",
        "server": server,
        "server_port": port,
        "password": password,
        "tls": tls_obj
    });

    // Port hopping: диапазон портов из authority (host:20000-30000) или из query ?mport=... / ?ports=...
    let mut effective_ranges = url_port_ranges.to_vec();
    if effective_ranges.is_empty() {
        if let Some(mport) = params.get("mport").or_else(|| params.get("ports")) {
            let (_, parsed_ranges) = parse_port_ranges(mport);
            effective_ranges = parsed_ranges;
        }
    }
    if !effective_ranges.is_empty() {
        if let Some(obj) = hy2_outbound.as_object_mut() {
            obj.remove("server_port");
        }
        hy2_outbound["server_ports"] = json!(effective_ranges);
    }

    // Обфускация (Salamander)
    let obfs = params.get("obfs").cloned();
    let obfs_password = params
        .get("obfs-password")
        .or_else(|| params.get("obfs_password"))
        .cloned();

    if let Some(obfs_type) = obfs {
        if !obfs_type.is_empty() {
            let mut obfs_obj = json!({ "type": obfs_type });
            if let Some(obfs_pw) = obfs_password {
                obfs_obj["password"] = json!(obfs_pw);
            }
            hy2_outbound["obfs"] = obfs_obj;
        }
    }

    // Скорости передачи
    if let Some(up) = params.get("upmbps").or_else(|| params.get("up_mbps")).and_then(|v| v.parse::<u32>().ok()) {
        hy2_outbound["up_mbps"] = json!(up);
    }
    if let Some(down) = params.get("downmbps").or_else(|| params.get("down_mbps")).and_then(|v| v.parse::<u32>().ok()) {
        hy2_outbound["down_mbps"] = json!(down);
    }

    let singbox_config = build_singbox_config(
        hy2_outbound,
        &server,
        routing_rules,
        app_data_dir,
        false,
    )?;

    Ok(ProxyConfig::SingBoxOnly(singbox_config))
}

/// Парсинг Hysteria 1 (hysteria://)
fn parse_hysteria1(
    parsed: &Url,
    routing_rules: Option<&RoutingRules>,
    app_data_dir: &Path,
) -> Result<ProxyConfig, String> {
    let raw_server = parsed
        .host_str()
        .ok_or_else(|| "Адрес сервера не найден в URL".to_string())?;
    let server = raw_server.trim_start_matches('[').trim_end_matches(']').to_string();

    let port = parsed.port().unwrap_or(443);

    let params: HashMap<String, String> = parsed
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    let auth_str = if !parsed.username().is_empty() {
        percent_decode(parsed.username())
    } else {
        params.get("auth").cloned().unwrap_or_default()
    };

    let sni = params
        .get("sni")
        .or_else(|| params.get("peer"))
        .cloned()
        .unwrap_or_else(|| server.clone());

    let insecure = params
        .get("insecure")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
        || params
            .get("allowInsecure")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

    let up_mbps = params
        .get("upmbps")
        .or_else(|| params.get("up_mbps"))
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(100);

    let down_mbps = params
        .get("downmbps")
        .or_else(|| params.get("down_mbps"))
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(100);

    let mut hy1_outbound = json!({
        "type": "hysteria",
        "tag": "proxy",
        "server": server,
        "server_port": port,
        "auth_str": auth_str,
        "up_mbps": up_mbps,
        "down_mbps": down_mbps,
        "tls": {
            "enabled": true,
            "server_name": sni,
            "insecure": insecure
        }
    });

    if let Some(obfs) = params.get("obfs").filter(|s| !s.is_empty()) {
        hy1_outbound["obfs"] = json!(obfs);
    }

    let singbox_config = build_singbox_config(
        hy1_outbound,
        &server,
        routing_rules,
        app_data_dir,
        false,
    )?;

    Ok(ProxyConfig::SingBoxOnly(singbox_config))
}

/// Единый генератор полного конфига sing-box (TUN, DNS, Маршрутизация, Монитор)
fn build_singbox_config(
    proxy_outbound: serde_json::Value,
    server: &str,
    routing_rules: Option<&RoutingRules>,
    app_data_dir: &Path,
    is_xray_chained: bool,
) -> Result<String, String> {
    let clean_server = server.trim_start_matches('[').trim_end_matches(']');
    let server_cidr = match clean_server.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => format!("{}/32", v4),
        Ok(std::net::IpAddr::V6(v6)) => format!("{}/128", v6),
        Err(_) => String::new(),
    };
    let is_ip = !server_cidr.is_empty();

    let mut proxy_outbound = proxy_outbound;
    if !is_ip && !is_xray_chained {
        proxy_outbound["domain_resolver"] = json!("local-dns");
    }

    let direct_outbound = json!({
        "type": "direct",
        "tag": "direct",
        "domain_resolver": "local-dns"
    });

    let mut config = json!({
        "log": {
            "disabled": false,
            "level": "info",
            "timestamp": true
        },
        "experimental": {
            "cache_file": {
                "enabled": true,
                "path": app_data_dir.join("cache.db").to_string_lossy().replace('\\', "/"),
                "store_rdrc": true
            },
            "clash_api": {
                "external_controller": "127.0.0.1:9090",
                "default_mode": "rule"
            }
        },
        "outbounds": [
            proxy_outbound,
            direct_outbound
        ]
    });

    let tun_inbound = if is_ip {
        json!({
            "type": "tun",
            "tag": "tun-in",
            "interface_name": "vlessok-tun",
            "address": ["172.28.240.1/30"],
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
            "address": ["172.28.240.1/30"],
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
            { "process_name": ["sing-box.exe", "xray.exe"], "server": "local-dns" },
            { "domain_suffix": [".local"], "server": "local-dns" }
        ],
        "final": "remote-dns",
        "strategy": "ipv4_only"
    });

    if !is_ip {
        if let Some(rules_array) = config["dns"]["rules"].as_array_mut() {
            rules_array.insert(0, json!({
                "domain": [server],
                "server": "local-dns"
            }));
        }
    }

    let mut route_rules = vec![
        json!({ "action": "sniff", "sniffer": ["http", "tls", "quic"], "timeout": "100ms" }),
        // Защита от петли до hijack-dns: DNS и трафик самих sing-box и xray всегда идут напрямую
        json!({ "process_name": ["sing-box.exe", "xray.exe"], "outbound": "direct" }),
        json!({ "port": [53], "action": "hijack-dns" }),
    ];

    // Явный bypass для сервера прокси
    if is_ip {
        route_rules.push(json!({
            "ip_cidr": [server_cidr],
            "outbound": "direct"
        }));
    } else {
        route_rules.push(json!({
            "domain": [server],
            "outbound": "direct"
        }));
    }

    // Приватные адреса идут через direct
    route_rules.push(json!({ "ip_is_private": true, "outbound": "direct" }));

    // Процессы по правилам пользователя
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

    let mut final_route = "proxy";
    let inside_path = app_data_dir.join("inside.json");
    let outside_path = app_data_dir.join("outside.json");

    let mut rule_sets = vec![];

    // Исключения перед блокировкой рекламы
    route_rules.push(json!({
        "domain_keyword": ["yandex", "metrika", "yastatic"],
        "outbound": "direct"
    }));

    // Блокировка рекламы
    rule_sets.push(json!({
        "tag": "geosite-category-ads-all",
        "type": "remote",
        "format": "binary",
        "url": "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-category-ads-all.srs",
        "download_detour": "proxy"
    }));
    route_rules.push(json!({
        "rule_set": ["geosite-category-ads-all"],
        "action": "reject"
    }));

    // outside-pack (прямой доступ)
    if outside_path.exists() {
        let p_str = outside_path.to_string_lossy().replace('\\', "/");
        rule_sets.push(json!({
            "tag": "outside-pack",
            "type": "local",
            "format": "source",
            "path": p_str
        }));
        route_rules.push(json!({
            "rule_set": ["outside-pack"],
            "outbound": "direct"
        }));
    }

    if let Some(rules) = routing_rules {
        if rules.routing_mode == RoutingMode::Rule {
            final_route = "direct";

            if !rules.domains.is_empty() {
                route_rules.push(json!({
                    "domain_suffix": rules.domains,
                    "outbound": "proxy"
                }));
            }

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
                        let url_domain = "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-telegram.srs";
                        rule_sets.push(json!({
                            "tag": "geosite-telegram",
                            "type": "remote",
                            "format": "binary",
                            "url": url_domain,
                            "download_detour": "proxy"
                        }));
                        proxy_tags.push("geosite-telegram".to_string());
                        route_rules.push(json!({
                            "process_name": ["telegram.exe", "Telegram.exe", "tg.exe", "tdata.exe"],
                            "outbound": "proxy"
                        }));
                        continue;
                    }

                    if geo_rule == "discord_combo" {
                        let url_domain = "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-discord.srs";
                        rule_sets.push(json!({
                            "tag": "geosite-discord",
                            "type": "remote",
                            "format": "binary",
                            "url": url_domain,
                            "download_detour": "proxy"
                        }));
                        proxy_tags.push("geosite-discord".to_string());
                        route_rules.push(json!({
                            "process_name": ["discord.exe", "Discord.exe", "Update.exe"],
                            "outbound": "proxy"
                        }));
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
                        "download_detour": "proxy"
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
        "default_domain_resolver": "local-dns",
        "rules": route_rules,
        "final": final_route
    });

    if !rule_sets.is_empty() {
        route_obj["rule_set"] = json!(rule_sets);
    }

    config["route"] = route_obj;

    let _ = is_xray_chained; // сохраняем для будущих кастомизаций

    serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Ошибка сериализации JSON sing-box: {}", e))
}

/// Базовая валидация UUID (8-4-4-4-12 hex символов)
fn validate_uuid(uuid: &str) -> Result<(), String> {
    if uuid.len() != 36 {
        return Err(format!(
            "Неверная длина UUID: {} символов (ожидается 36)",
            uuid.len()
        ));
    }

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
// Тесты (cargo test)
// ============================================================
#[cfg(test)]
mod tests {
    use super::*;

    const TEST_VLESS_URL: &str = "vless://550e8400-e29b-41d4-a716-446655440000@192.168.1.1:443\
        ?type=tcp&encryption=none&security=reality\
        &pbk=testPublicKey123456789012345678901234567890\
        &fp=chrome&sni=www.google.com&sid=abcdef12\
        &spx=%2F&flow=xtls-rprx-vision#MyServer";

    const TEST_XHTTP_URL: &str = "vless://550e8400-e29b-41d4-a716-446655440000@1.2.3.4:443\
        ?type=xhttp&path=%2Fcustom-path&mode=auto&security=reality\
        &pbk=testPublicKey123456789012345678901234567890\
        &fp=chrome&sni=www.google.com&sid=abcdef12#MyXhttpServer";

    const TEST_HYSTERIA2_URL: &str = "hysteria2://mySecretPass123@my.server.com:8443\
        ?sni=my.server.com&insecure=1&obfs=salamander&obfs-password=obfsPass123&upmbps=50&downmbps=100#MyHy2Server";

    const TEST_HY2_SHORT_URL: &str = "hy2://simplepass@1.2.3.4:443#QuickHy2";

    const TEST_HYSTERIA1_URL: &str = "hysteria://1.2.3.4:443\
        ?auth=hy1pass&sni=example.com&upmbps=30&downmbps=60#MyHy1";

    #[test]
    fn test_valid_vless_url_parses_ok() {
        let app_dir = Path::new("");
        let result = parse_proxy_url(TEST_VLESS_URL, None, &app_dir, 10808);
        assert!(result.is_ok());
        if let Ok(ProxyConfig::SingBoxOnly(json_str)) = result {
            assert!(json_str.contains("192.168.1.1"));
            assert!(json_str.contains("xtls-rprx-vision"));
        } else {
            panic!("Ожидался SingBoxOnly для стандартного VLESS");
        }
    }

    #[test]
    fn test_vless_xhttp_parses_to_xray_chained() {
        let app_dir = Path::new("");
        let result = parse_proxy_url(TEST_XHTTP_URL, None, &app_dir, 10808);
        assert!(result.is_ok());
        if let Ok(ProxyConfig::XrayChained { xray_config, singbox_config }) = result {
            // Xray должен содержать настройки xhttp и reality
            assert!(xray_config.contains("xhttp"));
            assert!(xray_config.contains("/custom-path"));
            assert!(xray_config.contains("reality"));
            assert!(xray_config.contains("10808"));

            // sing-box должен иметь outbound socks на 10808
            assert!(singbox_config.contains("\"type\": \"socks\""));
            assert!(singbox_config.contains("10808"));
            assert!(singbox_config.contains("vlessok-tun"));

            assert!(validate_with_xray(&xray_config), "Конфиг Xray должен успешно проходить валидацию xray.exe");
            assert!(validate_with_singbox(&singbox_config), "Конфиг sing-box (chained) должен успешно проходить sing-box.exe check");
        } else {
            panic!("Ожидался XrayChained для VLESS + XHTTP");
        }
    }

    #[test]
    fn test_vless_xhttp_tls_mode() {
        let app_dir = Path::new("");
        let url = "vless://550e8400-e29b-41d4-a716-446655440000@vpn.example.com:443\
            ?type=xhttp&path=%2Fstream&mode=stream-up&security=tls&sni=vpn.example.com#XhttpTls";
        let result = parse_proxy_url(url, None, &app_dir, 10809);
        assert!(result.is_ok());
        if let Ok(ProxyConfig::XrayChained { xray_config, singbox_config }) = result {
            assert!(xray_config.contains("\"stream-up\""));
            assert!(xray_config.contains("\"/stream\""));
            assert!(xray_config.contains("\"tls\""));
            assert!(validate_with_xray(&xray_config));
            assert!(validate_with_singbox(&singbox_config));
        } else {
            panic!("Ожидался XrayChained для VLESS + XHTTP TLS");
        }
    }

    #[test]
    fn test_vless_splithttp() {
        let app_dir = Path::new("");
        let url = "vless://550e8400-e29b-41d4-a716-446655440000@vpn.splithttp.com:443\
            ?type=splithttp&path=%2Fsplit&security=tls&sni=vpn.splithttp.com#SplitHttp";
        let result = parse_proxy_url(url, None, &app_dir, 10810);
        assert!(result.is_ok());
        if let Ok(ProxyConfig::XrayChained { xray_config, singbox_config }) = result {
            assert!(xray_config.contains("xhttp"));
            assert!(xray_config.contains("/split"));
            assert!(validate_with_xray(&xray_config));
            assert!(validate_with_singbox(&singbox_config));
        } else {
            panic!("Ожидался XrayChained для splithttp");
        }
    }

    #[test]
    fn test_vless_ws_and_grpc() {
        let app_dir = Path::new("");
        let ws_url = "vless://550e8400-e29b-41d4-a716-446655440000@1.2.3.4:443\
            ?type=ws&path=%2Fmyws&host=example.com&security=tls&sni=example.com#WsNode";
        let ws_res = parse_proxy_url(ws_url, None, &app_dir, 10808);
        assert!(ws_res.is_ok());
        if let Ok(ProxyConfig::SingBoxOnly(cfg)) = ws_res {
            assert!(cfg.contains("\"type\": \"ws\""));
            assert!(cfg.contains("/myws"));
            assert!(validate_with_singbox(&cfg));
        }

        let grpc_url = "vless://550e8400-e29b-41d4-a716-446655440000@1.2.3.4:443\
            ?type=grpc&serviceName=my-service&security=tls&sni=example.com#GrpcNode";
        let grpc_res = parse_proxy_url(grpc_url, None, &app_dir, 10808);
        assert!(grpc_res.is_ok());
        if let Ok(ProxyConfig::SingBoxOnly(cfg)) = grpc_res {
            assert!(cfg.contains("\"type\": \"grpc\""));
            assert!(cfg.contains("my-service"));
            assert!(validate_with_singbox(&cfg));
        }
    }

    #[test]
    fn test_ipv6_server_parses() {
        let app_dir = Path::new("");
        let url = "vless://550e8400-e29b-41d4-a716-446655440000@[2606:4700:4700::1111]:443\
            ?security=reality&pbk=testPublicKey123456789012345678901234567890&sni=cloudflare.com#Ipv6Node";
        let result = parse_proxy_url(url, None, &app_dir, 10808);
        assert!(result.is_ok());
        if let Ok(ProxyConfig::SingBoxOnly(cfg)) = result {
            assert!(cfg.contains("2606:4700:4700::1111/128"));
            assert!(validate_with_singbox(&cfg));
        }
    }

    #[test]
    fn test_hysteria2_parses_ok() {
        let app_dir = Path::new("");
        let result = parse_proxy_url(TEST_HYSTERIA2_URL, None, &app_dir, 10808);
        assert!(result.is_ok());
        if let Ok(ProxyConfig::SingBoxOnly(json_str)) = result {
            assert!(json_str.contains("\"type\": \"hysteria2\""));
            assert!(json_str.contains("mySecretPass123"));
            assert!(json_str.contains("salamander"));
            assert!(json_str.contains("obfsPass123"));
            assert!(json_str.contains("8443"));
            assert!(validate_with_singbox(&json_str), "Hysteria 2 конфиг должен проходить валидацию sing-box.exe");
        } else {
            panic!("Ожидался SingBoxOnly для Hysteria 2");
        }
    }

    #[test]
    fn test_hy2_short_scheme_parses_ok() {
        let app_dir = Path::new("");
        let result = parse_proxy_url(TEST_HY2_SHORT_URL, None, &app_dir, 10808);
        assert!(result.is_ok());
        if let Ok(ProxyConfig::SingBoxOnly(json_str)) = result {
            assert!(json_str.contains("\"type\": \"hysteria2\""));
            assert!(json_str.contains("simplepass"));
            assert!(json_str.contains("1.2.3.4"));
            assert!(validate_with_singbox(&json_str));
        } else {
            panic!("Ожидался SingBoxOnly для hy2://");
        }
    }

    #[test]
    fn test_hy2_percent_encoded_and_default_port() {
        let app_dir = Path::new("");
        let url = "hy2://pass%20with%20spaces%21@vpn.myhost.com#NoPortSpecified";
        let result = parse_proxy_url(url, None, &app_dir, 10808);
        assert!(result.is_ok());
        if let Ok(ProxyConfig::SingBoxOnly(json_str)) = result {
            assert!(json_str.contains("\"server_port\": 443"));
            assert!(json_str.contains("pass with spaces!"));
            assert!(validate_with_singbox(&json_str));
        }
    }

    #[test]
    fn test_hysteria1_parses_ok() {
        let app_dir = Path::new("");
        let result = parse_proxy_url(TEST_HYSTERIA1_URL, None, &app_dir, 10808);
        assert!(result.is_ok());
        if let Ok(ProxyConfig::SingBoxOnly(json_str)) = result {
            assert!(json_str.contains("\"type\": \"hysteria\""));
            assert!(json_str.contains("hy1pass"));
            assert!(json_str.contains("30"));
            assert!(json_str.contains("60"));
            assert!(validate_with_singbox(&json_str));
        } else {
            panic!("Ожидался SingBoxOnly для Hysteria 1");
        }
    }

    #[test]
    fn test_invalid_uuid_returns_error() {
        let app_dir = Path::new("");
        let bad_url = "vless://not-a-uuid@1.2.3.4:443?sni=x.com&pbk=key";
        let result = parse_proxy_url(bad_url, None, &app_dir, 10808);
        assert!(result.is_err());
    }

    #[test]
    fn test_custom_routing_rules() {
        let app_dir = Path::new("");
        let mut rules = RoutingRules::default();
        rules.routing_mode = RoutingMode::Rule;
        rules.domains = vec!["custom.domain.com".to_string()];
        rules.processes = vec!["customapp.exe".to_string()];
        rules.geo_rules = vec!["telegram_combo".to_string(), "discord_combo".to_string()];

        let result = parse_proxy_url(TEST_VLESS_URL, Some(&rules), &app_dir, 10808);
        assert!(result.is_ok());
        if let Ok(ProxyConfig::SingBoxOnly(json_str)) = result {
            assert!(json_str.contains("custom.domain.com"));
            assert!(json_str.contains("customapp.exe"));
            assert!(json_str.contains("telegram.exe"));
            assert!(json_str.contains("discord.exe"));
            assert!(validate_with_singbox(&json_str));
        }
    }

    #[test]
    fn test_hy2_pin_sha256_does_not_break_singbox() {
        let app_dir = Path::new("");
        let url = "hy2://pass@1.2.3.4:443?insecure=1&pinSHA256=abcdef123456#PinTest";
        let result = parse_proxy_url(url, None, &app_dir, 10808);
        assert!(result.is_ok());
        if let Ok(ProxyConfig::SingBoxOnly(json_str)) = result {
            assert!(!json_str.contains("certificate_pin"));
            assert!(validate_with_singbox(&json_str), "pinSHA256 не должен ронять sing-box");
        } else {
            panic!("Ожидался SingBoxOnly для Hysteria 2");
        }
    }

    #[test]
    fn test_hy2_port_hopping_authority_and_mport() {
        let app_dir = Path::new("");

        let url_auth = "hy2://pass@1.2.3.4:20000-30000?sni=example.com#Hop1";
        let res_auth = parse_proxy_url(url_auth, None, &app_dir, 10808).unwrap();
        if let ProxyConfig::SingBoxOnly(json_str) = res_auth {
            assert!(json_str.contains("\"server_ports\""), "Должен быть server_ports");
            assert!(json_str.contains("20000:30000"), "Диапазон должен быть в формате 20000:30000");
            assert!(!json_str.contains("\"server_port\""), "Не должно быть server_port при server_ports");
            assert!(validate_with_singbox(&json_str), "Port hopping authority должен проходить sing-box check");
        } else {
            panic!("Ожидался SingBoxOnly");
        }

        let url_mport = "hy2://pass@[2606:4700::1]:443?mport=443,20000-30000&sni=example.com#Hop2";
        let res_mport = parse_proxy_url(url_mport, None, &app_dir, 10808).unwrap();
        if let ProxyConfig::SingBoxOnly(json_str) = res_mport {
            assert!(json_str.contains("443:443"));
            assert!(json_str.contains("20000:30000"));
            assert!(!json_str.contains("\"server_port\""));
            assert!(validate_with_singbox(&json_str), "mport должен проходить sing-box check");
        } else {
            panic!("Ожидался SingBoxOnly");
        }
    }

    #[test]
    fn test_xhttp_insecure_without_pcs_returns_error() {
        let app_dir = Path::new("");
        let url = "vless://550e8400-e29b-41d4-a716-446655440000@vpn.example.com:443\
            ?type=xhttp&path=%2Fstream&security=tls&sni=vpn.example.com&insecure=1#BadTls";
        let result = parse_proxy_url(url, None, &app_dir, 10808);
        assert!(result.is_err(), "insecure=1 без pcs должен возвращать понятную ошибку");
    }

    #[test]
    fn test_xhttp_security_none_public_ip_returns_error() {
        let app_dir = Path::new("");
        let url = "vless://550e8400-e29b-41d4-a716-446655440000@1.2.3.4:443\
            ?type=xhttp&path=%2Fstream&security=none#PublicPlaintext";
        let result = parse_proxy_url(url, None, &app_dir, 10808);
        assert!(result.is_err(), "security=none до публичного адреса должен возвращать ошибку");

        let local_url = "vless://550e8400-e29b-41d4-a716-446655440000@127.0.0.1:8443\
            ?type=xhttp&path=%2Fstream&security=none#LocalPlaintext";
        let local_res = parse_proxy_url(local_url, None, &app_dir, 10808);
        assert!(local_res.is_ok(), "security=none для localhost/127.0.0.1 должен работать");
        if let Ok(ProxyConfig::XrayChained { xray_config, .. }) = local_res {
            assert!(validate_with_xray(&xray_config));
        }
    }

    #[test]
    fn test_digit_prefixed_domain_is_not_treated_as_ip() {
        let app_dir = Path::new("");
        let url = "vless://550e8400-e29b-41d4-a716-446655440000@1vpn.example.com:443\
            ?security=tls&sni=1vpn.example.com&alpn=h2,http/1.1&insecure=1#DigitDomain";
        let result = parse_proxy_url(url, None, &app_dir, 10808).unwrap();
        if let ProxyConfig::SingBoxOnly(cfg) = result {
            assert!(!cfg.contains("1vpn.example.com/32"), "Домен с цифры не должен считаться IP");
            assert!(cfg.contains("\"alpn\""));
            assert!(cfg.contains("\"insecure\": true"));
            assert!(validate_with_singbox(&cfg), "Конфиг с alpn и insecure должен проходить sing-box check");
        } else {
            panic!("Ожидался SingBoxOnly");
        }
    }

    #[test]
    fn test_xhttp_extra_and_alpn() {
        let app_dir = Path::new("");
        let url = "vless://550e8400-e29b-41d4-a716-446655440000@vpn.example.com:443\
            ?type=xhttp&path=%2Fx&security=tls&sni=vpn.example.com&alpn=h2&extra=%7B%22scMaxEachPostBytes%22%3A1000000%7D#ExtraTest";
        let result = parse_proxy_url(url, None, &app_dir, 10808).unwrap();
        if let ProxyConfig::XrayChained { xray_config, singbox_config } = result {
            assert!(xray_config.contains("scMaxEachPostBytes"), "extra JSON должен попадать в xhttpSettings");
            assert!(xray_config.contains("\"alpn\""), "alpn должен попадать в tlsSettings");
            assert!(validate_with_xray(&xray_config), "Xray должен принимать extra и alpn");
            assert!(validate_with_singbox(&singbox_config), "sing-box должен проходить check");
        } else {
            panic!("Ожидался XrayChained");
        }
    }

    fn validate_with_singbox(json_content: &str) -> bool {
        let bin_candidates = [
            Path::new("binaries").join("sing-box.exe"),
            Path::new("src-tauri").join("binaries").join("sing-box.exe"),
            Path::new("..").join("src-tauri").join("binaries").join("sing-box.exe"),
        ];
        let bin = bin_candidates.into_iter().find(|p| p.exists());
        let bin_path = match bin {
            Some(b) => b,
            None => return true,
        };

        static SB_CTR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = SB_CTR.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let temp_file = std::env::temp_dir().join(format!("vlessok-test-sb-{}-{}.json", std::process::id(), id));
        
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&temp_file).unwrap();
            f.write_all(json_content.as_bytes()).unwrap();
            f.flush().unwrap();
        }

        let output = std::process::Command::new(bin_path)
            .arg("check")
            .arg("-c")
            .arg(&temp_file)
            .output();

        let _ = std::fs::remove_file(temp_file);
        match output {
            Ok(o) => {
                if !o.status.success() {
                    eprintln!("sing-box check failed:\nSTDOUT: {}\nSTDERR: {}",
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr));
                }
                o.status.success()
            }
            Err(e) => {
                eprintln!("sing-box spawn error: {}", e);
                false
            }
        }
    }

    fn validate_with_xray(json_content: &str) -> bool {
        let bin_candidates = [
            Path::new("binaries").join("xray.exe"),
            Path::new("src-tauri").join("binaries").join("xray.exe"),
            Path::new("..").join("src-tauri").join("binaries").join("xray.exe"),
        ];
        let bin = bin_candidates.into_iter().find(|p| p.exists());
        let bin_path = match bin {
            Some(b) => b,
            None => return true,
        };

        static XR_CTR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = XR_CTR.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let temp_file = std::env::temp_dir().join(format!("vlessok-test-xr-{}-{}.json", std::process::id(), id));

        {
            use std::io::Write;
            let mut f = std::fs::File::create(&temp_file).unwrap();
            f.write_all(json_content.as_bytes()).unwrap();
            f.flush().unwrap();
        }

        let output = std::process::Command::new(bin_path)
            .arg("-test")
            .arg("-c")
            .arg(&temp_file)
            .output();

        let _ = std::fs::remove_file(temp_file);
        match output {
            Ok(o) => {
                if !o.status.success() {
                    eprintln!("xray test failed:\nSTDOUT: {}\nSTDERR: {}",
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr));
                }
                o.status.success()
            }
            Err(e) => {
                eprintln!("xray spawn error: {}", e);
                false
            }
        }
    }
}
