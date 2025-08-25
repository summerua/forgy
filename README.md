# Forgy 🗜️

**High-performance REST API load testing tool built in Rust with real-time Prometheus metrics**

forgy is a modern load testing tool designed to stress-test REST endpoints with precision and efficiency. Built with Rust for maximum performance, it leverages async I/O and multi-core processing to generate massive concurrent loads while maintaining detailed metrics and observability.

## Installation

### Quick Install (Recommended)

Install forgy with a single command:

```bash
curl -fsSL https://raw.githubusercontent.com/summerua/forgy/main/install.sh | bash
```

This will:
- Detect your platform (Linux/macOS, x86_64/ARM64)
- Download the latest release binary
- Install it to `~/.local/bin/forgy`
- Make it executable

After installation, you may need to add `~/.local/bin` to your PATH:
```bash
export PATH="$HOME/.local/bin:$PATH"
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
```

### Manual Download

Download pre-built binaries from [GitHub Releases](https://github.com/summerua/forgy/releases):

- **Linux (x86_64)**: `forgy-linux-x86_64`
- **macOS (x86_64)**: `forgy-macos-x86_64`  
- **macOS (ARM64)**: `forgy-macos-arm64`

```bash
# Example for Linux
wget https://github.com/summerua/forgy/releases/latest/download/forgy-linux-x86_64
chmod +x forgy-linux-x86_64
sudo mv forgy-linux-x86_64 /usr/local/bin/forgy
```

### Build from Source

If you prefer to build from source:

#### Prerequisites
- Rust 1.70 or higher
- Cargo (comes with Rust)

Install Rust from [https://rustup.rs/](https://rustup.rs/):
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

#### Build Steps
1. Clone the repository:
```bash
git clone https://github.com/summerua/forgy.git
cd forgy
```

2. Build and install:
```bash
# Install directly to cargo bin
cargo install --path .

# Or build manually
cargo build --release
./target/release/forgy --help
```

## Quick Start

```bash
# Simple load test with 100 virtual users for 5 minutes
forgy --url=http://localhost:3000/api --vus=100 --hold=5m

# Advanced test with ramp-up/down and Prometheus Remote Write
forgy --url=http://api.example.com/endpoint \
  --vus=1000 \
  --ramp-up=2m \
  --hold=10m \
  --ramp-down=1m \
  --prometheus-url=http://localhost:9090/api/v1/write \
  --app=api-test \
  --metrics-frequency=15

# POST request with custom headers and body
forgy --url=http://api.example.com/users \
  --method=POST \
  --header="Content-Type:application/json" \
  --header="Authorization:Bearer token" \
  --body='{"name":"test","email":"test@example.com"}' \
  --vus=500 \
  --hold=30m \
  --output=results.json
```

## Command Line Options

```
OPTIONS:
    --url <URL>                      Target URL to test [required]
    --vus <COUNT>                    Number of virtual users (default: 10)
    --ramp-up <DURATION>             Ramp-up duration (e.g., 5m, 30s) (default: 10s)
    --hold <DURATION>                Hold duration at peak load (default: 30s)
    --ramp-down <DURATION>           Ramp-down duration (default: 10s)
    --method <METHOD>                HTTP method (default: GET)
    --body <BODY>                    Request body for POST/PUT requests
    --header <HEADER>                Headers in "Key:Value" format (can be repeated)
    --timeout <SECONDS>              Request timeout in seconds (default: 30)
    --workers <COUNT>                Number of worker threads (default: CPU count)
    --output <FILE>                  Save results to JSON file
    --prometheus-url <URL>           Prometheus Remote Write URL (e.g., http://localhost:9090/api/v1/write)
    --app <LABEL>                    Application label for grouping metrics in Prometheus (default: forgy)
    --metrics-frequency <SECS>       Metrics push frequency in seconds (default: 10)
    --help                           Print help information
```

## Prometheus Integration

forgy supports **Remote Write** to send metrics directly to Prometheus, which is ideal for real-time load testing metrics.

When using `--prometheus-url`, forgy sends metrics to the specified Prometheus Remote Write endpoint. Metrics are sent every 10 seconds by default (configurable with `--metrics-frequency`).

### Setup

1. **Enable Remote Write in Prometheus:**
   Start Prometheus with the remote write receiver feature flag:
   ```bash
   prometheus --enable-feature=remote-write-receiver
   ```

2. **Verify Remote Write endpoint is available:**
   The endpoint will be available at `http://localhost:9090/api/v1/write`

3. **Run forgy with Remote Write:**
   ```bash
   forgy --url=http://api.example.com \
     --prometheus-url=http://localhost:9090/api/v1/write \
     --app=my-test
   ```

### Multiple Test Runs

Use different `--app` values to distinguish between different test runs:
```bash
# Frontend test
forgy --url=http://frontend.example.com \
  --prometheus-url=http://localhost:9090/api/v1/write \
  --app=frontend-test

# Backend test  
forgy --url=http://backend.example.com \
  --prometheus-url=http://localhost:9090/api/v1/write \
  --app=backend-test
```

Each test will send metrics with different job labels to the same Remote Write endpoint. The `app` value becomes the job name but does not modify the `prometheus-url`.

### Available Metrics

All metrics are prefixed with `forgy_` to distinguish them from other metrics:

- `forgy_requests_total` - Total requests by status and method
- `forgy_request_duration_seconds` - Request duration histogram  
- `forgy_active_vus` - Currently active virtual users
- `forgy_target_vus` - Target number of virtual users
- `forgy_success_rate` - Current success rate percentage
- `forgy_requests_per_second` - Current throughput
- `forgy_response_time_p50_ms` - 50th percentile response time
- `forgy_response_time_p90_ms` - 90th percentile response time
- `forgy_response_time_p95_ms` - 95th percentile response time
- `forgy_response_time_p99_ms` - 99th percentile response time
- `forgy_phase` - Current test phase (idle=1, ramp-up=1, hold=1, ramp-down=1)

## License

MIT