use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

pub fn config_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = dirs::config_dir()
        .ok_or("could not determine config directory")?
        .join("anonkeyst");
    Ok(dir.join("config.toml"))
}

pub fn key_exists() -> bool {
    config_path().ok().map(|p| p.exists()).unwrap_or(false)
}

pub fn save_key(key: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut table = toml::Table::new();
    table.insert("api_key".to_string(), toml::Value::String(key.to_string()));
    let content = toml::to_string(&table)?;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    file.write_all(content.as_bytes())?;
    Ok(())
}

pub fn load_key() -> Result<String, Box<dyn std::error::Error>> {
    let path = config_path()?;
    let content = fs::read_to_string(&path).map_err(|_| {
        format!(
            "no config found at {}. Run `anonkeyst register` first.",
            path.display()
        )
    })?;
    let table: toml::Table = content.parse()?;
    let key = table
        .get("api_key")
        .and_then(|v| v.as_str())
        .ok_or("api_key not found in config")?
        .to_string();
    Ok(key)
}
