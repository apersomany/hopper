# Hopper

A simple Minecraft proxy written in Rust that provides hostname-based routing and dynamic server registration.

## Features

- **Hostname Based Routing**: Route incoming Minecraft connections to different backend servers based on the hostname used
- **Dynamic Registration**: Register new routes via a simple HTTP API
- **Persistent Configuration**: Automatically saves routing configuration on graceful shutdown

## Installation

### Building from Source

```bash
git clone https://github.com/apersomany/hopper
cd hopper
cargo build --release
```

The compiled binary will be available at `target/release/hopper`.

## Configuration

### Configuration File

On startup, Hopper looks for a `config.json` file. If not found, it uses default settings.

Example `config.json`:

```json
{
	"routes": {
		"survival.example.com": "[::]:25566",
		"creative.example.com": "[::]:25567",
		"lobby.example.com": "[::]:25568"
	},
	"minecraft_proxy": "[::]:25565",
	"http_api_server": "[::]:80"
}
```

### Configuration Options

- **routes**: A map of hostnames to backend server addresses
- **minecraft_proxy**: The address to bind the Minecraft proxy listener (default: `[::]:25565`)
- **http_api_server**: The address to bind the HTTP API server (default: `[::]:80`)

## Usage

### Running the Server

```bash
./hopper
```

Or with cargo:

```bash
cargo run --release
```

### Dynamic Route Registration

Register a new route using the HTTP API:

```bash
curl http://localhost/register/mynewserver.example.com
```

This automatically registers the calling IP address on port 25565 as the backend for the specified hostname.

The HTTP API has no authentication - consider running it on a private network or adding authentication
