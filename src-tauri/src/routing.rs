use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RoutingMode {
    Global,
    Rule,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RoutingRules {
    #[serde(default = "default_routing_mode")]
    pub routing_mode: RoutingMode,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub geo_rules: Vec<String>,
    #[serde(default)]
    pub processes: Vec<String>,
}

fn default_routing_mode() -> RoutingMode {
    RoutingMode::Global
}

impl Default for RoutingRules {
    fn default() -> Self {
        Self {
            routing_mode: RoutingMode::Rule, // По умолчанию лучше Rule, чтобы Russia Pack работал сразу
            domains: Vec::new(),
            geo_rules: vec!["russia_pack".to_string()],
            processes: Vec::new(),
        }
    }
}

impl RoutingRules {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join("routing.json");
        if !path.exists() {
            return Ok(Self::default());
        }

        let data = match fs::read_to_string(&path) {
            Ok(d) => d,
            Err(e) => return Err(format!("Ошибка чтения файла routing.json: {}", e)),
        };

        match serde_json::from_str(&data) {
            Ok(rules) => Ok(rules),
            Err(_) => {
                // Если файл поврежден, бекапим его и возвращаем дефолтные правила
                let bak_path = dir.join("routing.json.bak");
                let _ = fs::copy(&path, &bak_path);
                Ok(Self::default())
            }
        }
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        if !dir.exists() {
            fs::create_dir_all(dir).map_err(|e| format!("Не удалось создать директорию: {}", e))?;
        }

        let path = dir.join("routing.json");
        let tmp_path = dir.join("routing.json.tmp");
        let bak_path = dir.join("routing.json.bak");

        let json_str = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Ошибка сериализации правил: {}", e))?;

        // Пишем во временный файл
        fs::write(&tmp_path, json_str)
            .map_err(|e| format!("Ошибка записи временного файла: {}", e))?;

        // Если основной файл есть, делаем бекап
        if path.exists() {
            let _ = fs::copy(&path, &bak_path);
        }

        // Атомарно заменяем основной файл временным
        fs::rename(&tmp_path, &path)
            .map_err(|e| format!("Ошибка при переименовании файла: {}", e))?;

        Ok(())
    }

    pub fn set_mode(&mut self, mode: RoutingMode) {
        self.routing_mode = mode;
    }

    pub fn add_domain(&mut self, input: &str) -> Result<String, String> {
        let domain = normalize_domain(input)?;
        if self.domains.contains(&domain) {
            return Err(format!("Домен '{}' уже в списке", domain));
        }
        self.domains.push(domain.clone());
        Ok(domain)
    }

    pub fn remove_domain(&mut self, domain: &str) {
        self.domains.retain(|d| d != domain);
    }

    pub fn add_geo_rule(&mut self, input: &str) -> Result<String, String> {
        let rule = input.trim().to_lowercase();
        if !rule.starts_with("geosite:") && !rule.starts_with("geoip:") 
            && rule != "russia_pack" && rule != "telegram_combo" && rule != "discord_combo" {
            return Err("Правило должно начинаться с 'geosite:' или 'geoip:' или быть известным пресетом".into());
        }
        if self.geo_rules.contains(&rule) {
            return Err(format!("Правило '{}' уже в списке", rule));
        }
        self.geo_rules.push(rule.clone());
        Ok(rule)
    }

    pub fn remove_geo_rule(&mut self, rule: &str) {
        self.geo_rules.retain(|r| r != rule);
    }

    pub fn add_process(&mut self, input: &str) -> Result<String, String> {
        let name = input.trim().to_string();
        if !name.to_lowercase().ends_with(".exe") {
            return Err("Имя процесса должно заканчиваться на '.exe'".into());
        }
        if self.processes.iter().any(|p| p.eq_ignore_ascii_case(&name)) {
            return Err(format!("Процесс '{}' уже в списке", name));
        }
        self.processes.push(name.clone());
        Ok(name)
    }

    pub fn remove_process(&mut self, name: &str) {
        self.processes.retain(|p| !p.eq_ignore_ascii_case(name));
    }
}

pub fn normalize_domain(input: &str) -> Result<String, String> {
    let mut s = input.trim().to_lowercase();

    // 1. Снимаем префиксы
    let prefixes = ["https://", "http://", "domain:", "full:"];
    for prefix in prefixes {
        if s.starts_with(prefix) {
            s = s[prefix.len()..].to_string();
            break; // Если нашли один префикс, сняли и дальше не ищем (обычно они не комбинируются)
        }
    }

    // Дополнительно: если кто-то ввел full:https://... (маловероятно, но на всякий случай)
    for prefix in prefixes {
        if s.starts_with(prefix) {
            s = s[prefix.len()..].to_string();
        }
    }

    // 2. Снимаем www.
    if s.starts_with("www.") {
        s = s[4..].to_string();
    }

    // 3. Срезаем всё после первого '/', '?' или '#'
    if let Some(pos) = s.find(|c| c == '/' || c == '?' || c == '#') {
        s = s[..pos].to_string();
    }

    // 4. Срезаем порт (после ':')
    if let Some(pos) = s.find(':') {
        s = s[..pos].to_string();
    }

    // 5. Проверки
    if s.is_empty() {
        return Err("Домен не может быть пустым".into());
    }

    if !s.contains('.') {
        return Err("Неверный формат домена (нет точки)".into());
    }

    // Проверяем недопустимые символы (оставляем буквы, цифры, дефис, точку, подчеркивание)
    // Строго говоря, домены содержат a-z, 0-9, -, . (иногда IDN, но для простоты валидируем мягко)
    for c in s.chars() {
        if !c.is_ascii_alphanumeric() && c != '-' && c != '.' && c != '_' {
            return Err(format!("Домен содержит недопустимый символ: '{}'", c));
        }
    }

    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_domain() {
        let cases = vec![
            ("youtube.com", "youtube.com"),
            ("  youtube.com  ", "youtube.com"),
            ("https://youtube.com", "youtube.com"),
            ("http://youtube.com", "youtube.com"),
            ("domain:youtube.com", "youtube.com"),
            ("full:youtube.com", "youtube.com"),
            ("www.youtube.com", "youtube.com"),
            ("WWW.GOOGLE.COM", "google.com"),
            ("https://www.youtube.com/watch?v=abc", "youtube.com"),
            ("youtube.com?utm=foo", "youtube.com"),
            ("youtube.com#section", "youtube.com"),
            ("https://login.microsoft.com:443/auth", "login.microsoft.com"),
            ("api.service.net", "api.service.net"),
        ];

        for (input, expected) in cases {
            assert_eq!(normalize_domain(input).unwrap(), expected, "Failed for input: {}", input);
        }
    }

    #[test]
    fn test_invalid_domains() {
        assert!(normalize_domain("localhost").is_err());
        assert!(normalize_domain("you tube.com").is_err()); // пробел внутри домена
    }
}
