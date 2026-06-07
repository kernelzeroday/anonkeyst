use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

static LITELLM_PID: AtomicU32 = AtomicU32::new(0);

fn install_cleanup_handler() {
    unsafe {
        libc::signal(libc::SIGINT, cleanup_on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, cleanup_on_signal as *const () as libc::sighandler_t);
    }
}

extern "C" fn cleanup_on_signal(_sig: libc::c_int) {
    let pid = LITELLM_PID.load(Ordering::Relaxed);
    if pid != 0 {
        unsafe { libc::kill(pid as i32, libc::SIGTERM); }
    }
    unsafe { libc::_exit(130); }
}

const BASE_URL: &str = "https://anonkey.st/v1";

pub const TOOLS: &[&str] = &["codex", "claude", "aider", "goose", "opencode", "copilot"];

pub fn run(tool: &str, api_key: &str, model: &str, extra_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    match tool {
        "codex" => launch_codex(api_key, model, extra_args),
        "claude" => launch_claude(api_key, model, extra_args),
        "aider" => launch_aider(api_key, model, extra_args),
        "goose" => launch_goose(api_key, model, extra_args),
        "opencode" => launch_opencode(api_key, model, extra_args),
        "copilot" => launch_copilot(api_key, model, extra_args),
        _ => Err(format!(
            "unknown tool '{}'. supported: {}",
            tool,
            TOOLS.join(", ")
        ).into()),
    }
}

fn find_binary(names: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    for name in names {
        if let Ok(path) = which::which(name) {
            return Ok(path.to_string_lossy().to_string());
        }
    }
    Err(format!(
        "'{}' not found in PATH. Install it first.",
        names[0]
    ).into())
}

const CODEX_PROXY_SCRIPT: &str = include_str!("codex_proxy.py");

fn write_codex_profile(port: u16, model: &str) -> Result<(), Box<dyn std::error::Error>> {
    let codex_home = dirs::home_dir()
        .ok_or("could not determine home directory")?
        .join(".codex");
    std::fs::create_dir_all(&codex_home)?;
    let profile_path = codex_home.join("anonkey.config.toml");

    let config = format!(
r#"model = "{model}"
model_provider = "anonkey"
model_reasoning_effort = "high"

[model_providers.anonkey]
name = "AnonKey"
base_url = "http://127.0.0.1:{port}"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
"#,
        model = model,
        port = port,
    );

    std::fs::write(&profile_path, &config)?;
    Ok(())
}

fn write_codex_auth(api_key: &str) -> Result<String, Box<dyn std::error::Error>> {
    let codex_home = dirs::home_dir()
        .ok_or("could not determine home directory")?
        .join(".codex");
    let auth_path = codex_home.join("auth.json");

    let backup = if auth_path.exists() {
        let content = std::fs::read_to_string(&auth_path)?;
        Some(content)
    } else {
        None
    };

    let auth = format!(r#"{{"auth_mode":"apikey","OPENAI_API_KEY":"{}"}}"#, api_key);
    std::fs::write(&auth_path, &auth)?;

    if let Some(bak) = backup {
        let bak_path = codex_home.join("auth.json.anonkey-bak");
        std::fs::write(&bak_path, &bak)?;
    }

    Ok(auth_path.to_string_lossy().to_string())
}

fn restore_codex_auth() {
    if let Some(home) = dirs::home_dir() {
        let bak = home.join(".codex/auth.json.anonkey-bak");
        let auth = home.join(".codex/auth.json");
        if bak.exists() {
            std::fs::rename(&bak, &auth).ok();
        }
    }
}

fn launch_codex(api_key: &str, model: &str, extra_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let codex_bin = find_binary(&["codex"])?;
    find_binary(&["python3"])?;

    let port = find_free_port()?;
    write_codex_profile(port, model)?;
    write_codex_auth(api_key)?;

    let proxy_dir = std::env::temp_dir().join("anonkeyst");
    std::fs::create_dir_all(&proxy_dir)?;
    let proxy_script = proxy_dir.join(format!("codex_proxy_{}.py", std::process::id()));
    std::fs::write(&proxy_script, CODEX_PROXY_SCRIPT)?;

    eprintln!("starting anonkey proxy on port {}...", port);

    let proxy = Command::new("python3")
        .arg(&proxy_script)
        .env("ANONKEY_API_KEY", api_key)
        .env("ANONKEY_PROXY_PORT", port.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut guard = LitellmGuard {
        child: Some(proxy),
        config_path: proxy_script.to_string_lossy().to_string(),
    };

    LITELLM_PID.store(guard.child.as_ref().unwrap().id(), Ordering::Relaxed);
    install_cleanup_handler();

    // Wait for proxy to be ready
    let start = Instant::now();
    let timeout = Duration::from_secs(10);
    let stderr = guard.child.as_mut().unwrap().stderr.take();
    let mut ready = false;
    if let Some(stderr) = stderr {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            if start.elapsed() > timeout { break; }
            if let Ok(line) = line {
                if line.contains("ready on port") {
                    ready = true;
                    break;
                }
            }
        }
    }
    if !ready {
        // fallback port poll
        let poll_start = Instant::now();
        while poll_start.elapsed() < Duration::from_secs(5) {
            if std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
                ready = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    if !ready {
        restore_codex_auth();
        return Err("anonkey proxy failed to start".into());
    }

    eprintln!("proxy ready, launching codex...");

    let mut codex = Command::new(&codex_bin)
        .arg("--profile").arg("anonkey")
        .args(extra_args)
        .env("OPENAI_API_KEY", api_key)
        .env_remove("OPENAI_BASE_URL")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_BASE_URL")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;

    let status = codex.wait()?;
    drop(guard);
    restore_codex_auth();

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn find_litellm_cmd() -> Result<(String, Vec<String>), Box<dyn std::error::Error>> {
    for name in &["litellm"] {
        if let Ok(path) = which::which(name) {
            return Ok((path.to_string_lossy().to_string(), vec![]));
        }
    }
    if let Ok(uvx) = which::which("uvx") {
        return Ok((uvx.to_string_lossy().to_string(), vec!["litellm[proxy]".to_string()]));
    }
    Err("'litellm' not found. Install with: uv tool install 'litellm[proxy]'".into())
}

struct LitellmGuard {
    child: Option<std::process::Child>,
    config_path: String,
}

impl Drop for LitellmGuard {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.child {
            child.kill().ok();
            child.wait().ok();
        }
        std::fs::remove_file(&self.config_path).ok();
    }
}

fn launch_claude(api_key: &str, model: &str, extra_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (litellm_bin, litellm_prefix_args) = find_litellm_cmd()?;
    let claude_bin = find_binary(&["claude"])?;

    let port = find_free_port()?;
    let proxy_url = format!("http://127.0.0.1:{}", port);

    let config = write_litellm_config(api_key, model)?;

    eprintln!("starting litellm proxy on port {}...", port);

    let mut litellm_args = litellm_prefix_args;
    litellm_args.extend([
        "--config".to_string(),
        config.clone(),
        "--port".to_string(),
        port.to_string(),
        "--host".to_string(),
        "127.0.0.1".to_string(),
    ]);

    let litellm = Command::new(&litellm_bin)
        .args(&litellm_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut guard = LitellmGuard {
        child: Some(litellm),
        config_path: config,
    };

    LITELLM_PID.store(guard.child.as_ref().unwrap().id(), Ordering::Relaxed);
    install_cleanup_handler();

    let ready = wait_for_litellm(guard.child.as_mut().unwrap(), port);
    if !ready {
        return Err("litellm proxy failed to start. Install with: uv tool install 'litellm[proxy]'".into());
    }

    eprintln!("litellm ready, launching claude...");

    let mut claude = Command::new(&claude_bin)
        .args(extra_args)
        .env("ANTHROPIC_BASE_URL", &proxy_url)
        .env("ANTHROPIC_API_KEY", "sk-litellm")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_BASE_URL")
        .env_remove("DISABLE_PROMPT_CACHING")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;

    let status = claude.wait()?;
    drop(guard);

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn write_litellm_config(api_key: &str, model: &str) -> Result<String, Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join("anonkeyst");
    std::fs::create_dir_all(&dir)?;
    let filename = format!("litellm-{}.yaml", std::process::id());
    let path = dir.join(filename);

    let config = format!(
r#"model_list:
  - model_name: "claude-sonnet-4-20250514"
    litellm_params:
      model: openai/{model}
      api_key: "{api_key}"
      api_base: "{BASE_URL}"
  - model_name: "claude-opus-4-20250514"
    litellm_params:
      model: openai/{model}
      api_key: "{api_key}"
      api_base: "{BASE_URL}"
  - model_name: "claude-haiku-4-20250514"
    litellm_params:
      model: openai/{model}
      api_key: "{api_key}"
      api_base: "{BASE_URL}"
  - model_name: "anthropic/*"
    litellm_params:
      model: openai/{model}
      api_key: "{api_key}"
      api_base: "{BASE_URL}"
  - model_name: "*"
    litellm_params:
      model: openai/{model}
      api_key: "{api_key}"
      api_base: "{BASE_URL}"

general_settings:
  master_key: "sk-litellm"
  drop_params: true

litellm_settings:
  drop_params: true
  modify_params: true
"#,
        model = model,
        api_key = api_key,
        BASE_URL = BASE_URL,
    );

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    file.write_all(config.as_bytes())?;
    Ok(path.to_string_lossy().to_string())
}

fn find_free_port() -> Result<u16, Box<dyn std::error::Error>> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn wait_for_litellm(child: &mut std::process::Child, port: u16) -> bool {
    let stderr = child.stderr.take();
    let stdout = child.stdout.take();
    let start = Instant::now();
    let timeout = Duration::from_secs(30);

    let check_lines = |reader: BufReader<Box<dyn std::io::Read + Send>>| -> bool {
        for line in reader.lines() {
            if start.elapsed() > timeout {
                return false;
            }
            match line {
                Ok(line) => {
                    if line.contains("Uvicorn running") || line.contains("Application startup complete") || line.contains("LiteLLM Proxy started") {
                        return true;
                    }
                    if line.contains("ERROR") {
                        eprintln!("  litellm: {}", line);
                    }
                }
                Err(_) => break,
            }
        }
        false
    };

    // Merge stdout and stderr into one stream to check
    if let Some(stderr) = stderr {
        if let Some(stdout) = stdout {
            // Spawn a thread to read stdout, check stderr on main
            let start_clone = start;
            let handle = std::thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    if start_clone.elapsed() > Duration::from_secs(30) {
                        return false;
                    }
                    match line {
                        Ok(line) => {
                            if line.contains("Uvicorn running") || line.contains("Application startup complete") || line.contains("LiteLLM Proxy started") {
                                return true;
                            }
                        }
                        Err(_) => break,
                    }
                }
                false
            });

            let reader = BufReader::new(Box::new(stderr) as Box<dyn std::io::Read + Send>);
            if check_lines(reader) {
                return true;
            }
            if let Ok(true) = handle.join() {
                return true;
            }
        } else {
            let reader = BufReader::new(Box::new(stderr) as Box<dyn std::io::Read + Send>);
            if check_lines(reader) {
                return true;
            }
        }
    }

    // fallback: poll the port
    let poll_start = Instant::now();
    while poll_start.elapsed() < Duration::from_secs(10) {
        if std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

fn launch_aider(api_key: &str, model: &str, extra_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let bin = find_binary(&["aider"])?;
    let mut cmd = Command::new(&bin);
    cmd.arg("--model").arg(format!("openai/{}", model));
    cmd.arg("--openai-api-base").arg(BASE_URL);
    cmd.args(extra_args);
    cmd.env("OPENAI_API_KEY", api_key);
    cmd.env("OPENAI_API_BASE", BASE_URL);
    Err(cmd.exec().into())
}

fn launch_goose(api_key: &str, model: &str, extra_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let bin = find_binary(&["goose", "goose-tui"])?;
    let mut cmd = Command::new(&bin);
    cmd.args(extra_args);
    cmd.env("GOOSE_PROVIDER", "openai");
    cmd.env("GOOSE_MODEL", model);
    cmd.env("OPENAI_API_KEY", api_key);
    cmd.env("OPENAI_HOST", "https://anonkey.st");
    cmd.env("OPENAI_BASE_PATH", "v1");
    Err(cmd.exec().into())
}

fn launch_opencode(api_key: &str, model: &str, extra_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let bin = find_binary(&["opencode"])?;
    let config = serde_json::json!({
        "provider": {
            "openai": {
                "apiKey": api_key,
                "baseURL": BASE_URL
            }
        },
        "agents": {
            "default": {
                "model": format!("openai/{}", model)
            }
        }
    });
    let mut cmd = Command::new(&bin);
    cmd.args(extra_args);
    cmd.env("OPENCODE_CONFIG_CONTENT", config.to_string());
    cmd.env("OPENAI_API_KEY", api_key);
    cmd.env("OPENAI_BASE_URL", BASE_URL);
    Err(cmd.exec().into())
}

fn launch_copilot(api_key: &str, model: &str, extra_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let bin = find_binary(&["github-copilot", "copilot"])?;
    let mut cmd = Command::new(&bin);
    cmd.args(extra_args);
    cmd.env("COPILOT_PROVIDER_BASE_URL", BASE_URL);
    cmd.env("COPILOT_PROVIDER_API_KEY", api_key);
    cmd.env("COPILOT_PROVIDER_WIRE_API", "responses");
    cmd.env("COPILOT_MODEL", model);
    Err(cmd.exec().into())
}
