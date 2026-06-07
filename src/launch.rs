use std::io::{BufRead, BufReader};
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
    std::process::exit(130);
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

fn launch_codex(api_key: &str, model: &str, extra_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let bin = find_binary(&["codex"])?;
    let mut cmd = Command::new(&bin);
    cmd.args(extra_args);
    cmd.env("OPENAI_API_KEY", api_key);
    cmd.env("OPENAI_BASE_URL", BASE_URL);
    cmd.env("CODEX_MODEL", model);
    Err(cmd.exec().into())
}

fn find_litellm_cmd() -> Result<(String, Vec<String>), Box<dyn std::error::Error>> {
    for name in &["litellm", "litellm-proxy"] {
        if let Ok(path) = which::which(name) {
            return Ok((path.to_string_lossy().to_string(), vec![]));
        }
    }
    if let Ok(uvx) = which::which("uvx") {
        return Ok((uvx.to_string_lossy().to_string(), vec!["litellm[proxy]".to_string()]));
    }
    Err("'litellm' not found. Install with: uv tool install 'litellm[proxy]'".into())
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

    let mut litellm = Command::new(&litellm_bin)
        .args(&litellm_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    LITELLM_PID.store(litellm.id(), Ordering::Relaxed);
    install_cleanup_handler();

    let ready = wait_for_litellm(&mut litellm, port);
    if !ready {
        litellm.kill().ok();
        return Err("litellm proxy failed to start. Install with: uv tool install 'litellm[proxy]'".into());
    }

    eprintln!("litellm ready, launching claude...");

    let mut claude = Command::new(&claude_bin)
        .args(extra_args)
        .env("ANTHROPIC_BASE_URL", &proxy_url)
        .env("ANTHROPIC_API_KEY", "sk-litellm")
        .env("ANTHROPIC_AUTH_TOKEN", "sk-litellm")
        .env("DISABLE_PROMPT_CACHING", "true")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;

    let status = claude.wait()?;
    litellm.kill().ok();
    std::fs::remove_file(&config).ok();

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn write_litellm_config(api_key: &str, model: &str) -> Result<String, Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join("anonkeyst");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("litellm-config.yaml");

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
  - model_name: "claude-sonnet-4-20250514"
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

    std::fs::write(&path, config)?;
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
    let start = Instant::now();
    let timeout = Duration::from_secs(30);

    if let Some(stderr) = stderr {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            if start.elapsed() > timeout {
                return false;
            }
            match line {
                Ok(line) => {
                    if line.contains("Uvicorn running") || line.contains("Application startup complete") {
                        return true;
                    }
                    if line.contains("ERROR") || line.contains("error") {
                        eprintln!("  litellm: {}", line);
                    }
                }
                Err(_) => break,
            }
        }
    }

    // fallback: poll the port
    let poll_start = Instant::now();
    while poll_start.elapsed() < Duration::from_secs(5) {
        if std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn launch_aider(api_key: &str, model: &str, extra_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let bin = find_binary(&["aider"])?;
    let mut cmd = Command::new(&bin);
    cmd.arg("--model").arg(format!("openai/{}", model));
    cmd.arg("--openai-api-base").arg(BASE_URL);
    cmd.arg("--openai-api-key").arg(api_key);
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
    cmd.env("OPENAI_BASE_PATH", "v1/chat/completions");
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
