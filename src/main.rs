mod client;
mod config;
mod launch;

use clap::{Parser, Subcommand};
use colored::Colorize;

#[derive(Parser)]
#[command(name = "anonkeyst", about = "CLI for anonkey.st — anonymous OpenAI proxy")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new anonymous account and store the API key
    Register,
    /// Show the stored API key
    Key,
    /// Check your current spendable balance
    Balance,
    /// List available models
    Models,
    /// Get a crypto deposit address to fund your account (defaults to XMR)
    Fund {
        /// Asset to deposit (e.g. BTC, XMR, ETH, USDT)
        #[arg(default_value = "XMR")]
        asset: String,
        /// Network for the deposit (e.g. bitcoin, monero, ethereum, tron)
        #[arg(default_value = "monero")]
        network: String,
    },
    /// List supported deposit asset/network pairs
    DepositPolicies,
    /// Send a one-shot chat completion
    Chat {
        /// Model to use (default: gpt-5.5)
        #[arg(short, long, default_value = "gpt-5.5")]
        model: String,
        /// The message to send
        message: String,
    },
    /// Launch an AI tool pre-configured to use anonkey.st
    Launch {
        /// Tool to launch: codex, claude, aider, goose, opencode, copilot
        tool: String,
        /// Model to use
        #[arg(short, long, default_value = "gpt-5.5")]
        model: String,
        /// Extra arguments passed to the tool
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli).await {
        eprintln!("{} {}", "error:".red().bold(), e);
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Commands::Register => {
            let c = client::Client::unauthenticated();
            let key = c.create_account().await?;
            config::save_key(&key)?;
            println!("{} Account created.", "✓".green().bold());
            println!("  API key: {}", key.bright_yellow());
            println!("  Saved to: {}", config::config_path()?.display());
        }
        Commands::Key => {
            let key = config::load_key()?;
            println!("{}", key);
        }
        Commands::Balance => {
            let c = authenticated_client()?;
            let b = c.get_balance().await?;
            println!("  {} ${:.4}", "Balance:".bold(), b.balance_usd);
            println!("  {} ${:.4}", "Spent:".bold(), b.total_spent_usd);
            println!("  {} ${:.4}", "Saved:".bold(), b.total_saved_usd);
            println!("  {} {}", "Key:".bold(), b.key_prefix);
        }
        Commands::Models => {
            let c = authenticated_client()?;
            let models = c.list_models().await?;
            for m in models {
                println!("  {}", m);
            }
        }
        Commands::Fund { asset, network } => {
            let c = authenticated_client()?;
            let addr = c.create_deposit_destination(&asset, &network).await?;
            println!("{} Deposit address created", "✓".green().bold());
            println!("  Asset:   {}", asset.bright_cyan());
            println!("  Network: {}", network.bright_cyan());
            println!("  Address: {}", addr.bright_yellow());
        }
        Commands::DepositPolicies => {
            let c = authenticated_client()?;
            let policies = c.get_deposit_policies().await?;
            println!("{}", "Supported deposit methods:".bold());
            for p in policies {
                println!("  {} on {}", p.asset.bright_cyan(), p.network);
            }
        }
        Commands::Chat { model, message } => {
            let c = authenticated_client()?;
            let reply = c.chat(&model, &message).await?;
            println!("{}", reply);
        }
        Commands::Launch { tool, model, args } => {
            let key = config::load_key()?;
            println!(
                "{} Launching {} with model {}",
                "→".bright_cyan().bold(),
                tool.bright_green(),
                model.bright_yellow()
            );
            launch::run(&tool, &key, &model, &args)?;
        }
    }
    Ok(())
}

fn authenticated_client() -> Result<client::Client, Box<dyn std::error::Error>> {
    let key = config::load_key()?;
    Ok(client::Client::new(&key))
}
