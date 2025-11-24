# Sheepnet

A Discord bot that posts daily Guild Wars game activities (Zaishen missions, bounties, etc.) and weekly bonuses.

## Overview

This bot:
- Connects to Discord and monitors for the daily 16:00 UTC time
- Fetches daily and weekly Guild Wars activities from the official wiki
- Posts formatted information to a specified Discord channel
- Automatically schedules the next post 24 hours later

## Features

- **Multiple output formats**: Discord embeds, plain text, Markdown, or HTML
- **Exponential backoff retry**: Automatically retries on HTTP errors (403, 500, etc.)
- **Time simulation**: Test with `--at-time` to verify behavior at specific times
- **Correct activity timing**: 
  - Nicholas Sandford updates at 07:00 UTC
  - Regular dailies update at 16:00 UTC
  - Weekly activities update at 15:00 UTC (Mondays)
- **Statically compiled**: Single binary with no dependencies (musl build)

## Requirements

- Rust 1.70 or newer
- A Discord bot token
- A Discord channel ID where messages will be posted

## Setup

1. **Clone and build the project:**

```bash
cd sheepnet
cargo build --release
```

2. **Create a Discord bot:**
   - Go to [Discord Developer Portal](https://discord.com/developers/applications)
   - Create a new application
   - Go to the "Bot" section and create a bot
   - Copy the bot token
   - Enable "Message Content Intent" under Privileged Gateway Intents

3. **Invite the bot to your server:**
   - Go to OAuth2 > URL Generator
   - Select scopes: `bot`
   - Select permissions: `Send Messages`, `Embed Links`
   - Use the generated URL to invite the bot

4. **Get your channel ID:**
   - Enable Developer Mode in Discord (User Settings > Advanced)
   - Right-click on the target channel and select "Copy ID"

## Running

### Discord Mode (Default)

Set the required environment variables and run:

```bash
export TOKEN="your-discord-bot-token"
export CHANNEL_ID="your-channel-id"
cargo run --release
```

Or use command-line arguments:

```bash
export TOKEN="your-discord-bot-token"
cargo run --release -- --discord-channel-id YOUR_CHANNEL_ID
```

### Command-Line Options

```bash
sheepnet [OPTIONS]

Options:
  --loop                      Run in loop mode (keep running daily) [default: false]
  --now                       Run immediately instead of waiting until 16:00 UTC
  --discord-channel-id <ID>   Discord channel ID (overrides CHANNEL_ID env var)
  --output-format <FORMAT>    Output format [default: discord]
                              [possible values: discord, txt, md, html]
  --at-time <TIME>            Simulate a specific time (YYYY-MM-DDTHH:MM:SS)
  --auto-update               Enable automatic updates from GitHub releases
  --version                   Print version information
  -h, --help                  Print help
```

### Testing and Debugging Examples

**Get immediate text output (no Discord):**

```bash
cargo run --release -- --now --output-format txt
```

**Get markdown output:**

```bash
cargo run --release -- --now --output-format md
```

**Get HTML output:**

```bash
cargo run --release -- --now --output-format html > output.html
```

**Test with simulated time:**

```bash
# Test before Nicholas Sandford updates (07:00 UTC)
cargo run --release -- --at-time 2025-11-25T06:59:00 --output-format txt

# Test after dailies update (16:00 UTC)
cargo run --release -- --at-time 2025-11-25T16:00:00 --output-format txt
```

**Normal Discord bot operation (wait until 16:00 UTC, then loop):**

```bash
export TOKEN="your-token"
export CHANNEL_ID="your-channel-id"
cargo run --release -- --loop
```

**Enable automatic updates:**

```bash
export TOKEN="your-token"
export CHANNEL_ID="your-channel-id"
cargo run --release -- --loop --auto-update
```

## Auto-Update Feature

The bot can automatically update itself from GitHub releases when run with the `--auto-update` flag.

### How It Works

1. **Daily Check**: At 15:00 UTC (one hour before posting), the bot checks GitHub for new releases
2. **Version Comparison**: Compares current version with the latest release tag
3. **Download & Verify**: Downloads the new binary and SHA256 checksum
4. **Security**: Verifies the checksum before replacing the binary
5. **Atomic Update**: Replaces the running binary atomically (no downtime)
6. **Restart**: Exits with code 42 to signal systemd/supervisor to restart

### Requirements

- Bot must have write permissions to its own binary file
- `sha256sum` command must be available
- Configured restart policy (e.g., systemd `Restart=always`)

### SystemD Configuration Example

```ini
[Unit]
Description=Sheepnet Guild Wars Discord Bot
After=network.target

[Service]
Type=simple
User=sheepnet
WorkingDirectory=/opt/sheepnet
Environment="TOKEN=your-token-here"
Environment="CHANNEL_ID=your-channel-id-here"
ExecStart=/usr/local/bin/sheepnet-linux-x86_64 --loop --auto-update
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

### SELinux Considerations

On systems with SELinux (like Fedora), the bot automatically attempts to copy the security context from the old binary to the new one using `chcon --reference`. If you encounter permission issues after an update, run:

```bash
restorecon -v /path/to/sheepnet-linux-x86_64
```

### Security Notes

- Only downloads releases from the repository specified in `Cargo.toml`
- Always verifies SHA256 checksums before installation
- Creates a `.backup` file of the old binary
- If update fails, the bot continues running with the current version
- Exit code 42 signals successful update and restart

## Testing

Run the unit tests:

```bash
cargo test
```

The tests use real HTML fixtures downloaded from the Guild Wars wiki to ensure parsing works correctly.

## Error Handling

The bot includes robust error handling with **exponential backoff retry logic**:

### HTTP Errors with Automatic Retry
- **403 Forbidden / 404 Not Found / 5xx Server Errors**: 
  - Error is logged with status code
  - **Automatic retry** with exponential backoff: 1s, 2s, 4s, 8s, 16s, 32s, 64s, 128s, 256s, then 300s (5min max)
  - Continues retrying until success (useful for temporary wiki outages)
  - No manual intervention needed

- **Network Errors** (timeout, DNS failure, connection refused):
  - Error is logged with descriptive message
  - Same exponential backoff retry logic applies
  - Bot will eventually recover when network is restored

### Retry Behavior Example
```
Daily activities returned HTTP 503 - retrying in 1s
Daily activities returned HTTP 503 - retrying in 2s
Daily activities returned HTTP 503 - retrying in 4s
Daily activities returned HTTP 503 - retrying in 8s
[Success after 15 seconds total]
```

**Time to reach maximum backoff**: ~8.5 minutes (1+2+4+8+16+32+64+128+256 seconds)  
**After maximum**: Retries every 5 minutes indefinitely until success

### Parsing Errors
- **Date not found in wiki tables**:
  - Error logged: "No daily/weekly data found for [date]"
  - May indicate wiki structure has changed
  - Update HTML fixtures and verify selectors still work

### Discord Errors
- **Failed to send message**:
  - Check bot permissions (Send Messages, Embed Links)
  - Verify channel ID is correct
  - Check TOKEN is valid

## Static Build

Build a statically linked binary with no dependencies:

```bash
cargo build --release --target x86_64-unknown-linux-musl
```

The binary will be at: `target/x86_64-unknown-linux-musl/release/sheepnet`

This can run on any Linux system without requiring installed libraries.

## Activity Update Times

Different activities update at different times:

- **Nicholas Sandford**: 07:00 UTC
- **Regular Dailies** (Zaishen Mission/Bounty/Combat/Vanquish, Wanted, Vanguard): 16:00 UTC
- **Weekly Activities** (PvE/PvP Bonus, Nicholas Location): 15:00 UTC (Mondays)

The bot correctly handles these different update times.

## License

This project is licensed under the GNU Affero General Public License v3.0 or later (AGPL-3.0-or-later).

See the [LICENSE](LICENSE) file for details.

### Why AGPL?

The AGPL ensures that if anyone runs this bot as a service (even without distributing the code), 
they must make the source code available to users of that service. This keeps the project open 
and benefits the community.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## Releases

Releases are automated via GitHub Actions. To create a new release:

```bash
# Tag the version
git tag v0.1.0
git push origin v0.1.0
```

The CI will automatically:
- Build a statically linked binary
- Strip debug symbols
- Create checksums
- Upload as release assets

Download pre-built binaries from the [Releases page](https://github.com/yourusername/sheepnet/releases).
