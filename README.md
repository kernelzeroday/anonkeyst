# anonkeyst

Rust CLI for [anonkey.st](https://anonkey.st) — anonymous, crypto-funded OpenAI proxy.

## Install

```bash
cargo install --git https://github.com/kernelzeroday/anonkeyst
```

## Usage

### Account Management

```bash
# Create a new anonymous account (key saved to ~/.config/anonkeyst/config.toml)
anonkeyst register

# Show your stored API key
anonkeyst key

# Check balance
anonkeyst balance
```

### Funding

```bash
# Get a Monero deposit address (default)
anonkeyst fund

# Get address for other assets
anonkeyst fund BTC bitcoin
anonkeyst fund ETH ethereum
anonkeyst fund USDT tron
anonkeyst fund SOL solana

# List all supported deposit methods
anonkeyst deposit-policies
```

### Chat

```bash
# One-shot chat (defaults to gpt-5.5)
anonkeyst chat "explain monads"

# Use a different model
anonkeyst chat -m gpt-4o "hello"
```

### Launch AI Tools

Launch coding tools pre-configured to route through anonkey.st:

```bash
# Launch OpenAI Codex CLI
anonkeyst launch codex

# Launch Claude Code (via Anthropic→OpenAI translation proxy)
anonkeyst launch claude

# Launch Aider
anonkeyst launch aider

# Launch with a specific model
anonkeyst launch codex -m gpt-4o

# Pass extra args to the tool
anonkeyst launch codex -- exec "say hello"
```

#### Supported Tools

| Tool | Method | Requirements |
|------|--------|-------------|
| `codex` | Embedded Python proxy (strips hosted tools, converts SSE) | `python3`, `codex` |
| `claude` | Embedded Python proxy (Anthropic Messages→OpenAI Chat translation) | `python3`, `claude` |
| `aider` | Direct (OpenAI-compatible natively) | `aider` |
| `goose` | Direct (OpenAI provider env vars) | `goose` |
| `opencode` | Direct (config via env var) | `opencode` |
| `copilot` | Direct (provider env vars) | `github-copilot` |

#### How Launch Works

**Direct tools** (aider, goose, opencode, copilot): Sets env vars pointing at `https://anonkey.st/v1` and `exec()`s the tool. No proxy needed — these tools speak OpenAI format natively.

**Codex**: Requires a local proxy because:
1. Codex sends `web_search`, `tool_search`, and `custom` hosted tool types that anonkey.st rejects with streaming
2. The proxy strips these tools and converts non-streaming JSON responses into the SSE event stream codex expects
3. Auth is temporarily swapped to `apikey` mode (restored on exit)

**Claude**: Embedded Python proxy translates Anthropic Messages API → OpenAI Chat Completions, including tool use conversion and SSE streaming. Launched with `--bare` to skip stored OAuth and use only the API key env var. No external dependencies beyond `python3`.

### Models

```bash
# List available models
anonkeyst models
```

Currently available: gpt-5.5, gpt-5.4, gpt-4o, gpt-4o-mini, gpt-image-1/2, embeddings.

## Configuration

Config is stored at `~/.config/anonkeyst/config.toml` (macOS: `~/Library/Application Support/anonkeyst/config.toml`) with `0600` permissions.

```toml
api_key = "sk-anonkey-..."
```

## Supported Cryptocurrencies

BTC, XMR, ETH, LTC, BCH, SOL, TON, TRX, BNB, USDT (ethereum/bsc/tron/solana/ton), USDC (ethereum/bsc/solana).

Default for `fund` is XMR (Monero) — credited after 1 confirmation (~2-5 minutes).

## Dependencies

- `python3` — required for `launch codex` and `launch claude` (embedded translation proxies)
