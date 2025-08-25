#!/bin/bash
set -e

# Forgy installation script
# Usage: curl -fsSL https://raw.githubusercontent.com/summerua/forgy/main/install.sh | bash

REPO="summerua/forgy"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Logging functions
info() { echo -e "${GREEN}[INFO]${NC} $*"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*" >&2; }

# Detect OS and architecture
detect_platform() {
    local os arch
    os=$(uname -s | tr '[:upper:]' '[:lower:]')
    arch=$(uname -m)
    
    case "$os" in
        linux) os="linux" ;;
        darwin) os="macos" ;;
        *) error "Unsupported OS: $os"; exit 1 ;;
    esac
    
    case "$arch" in
        x86_64|amd64) arch="x86_64" ;;
        arm64|aarch64) arch="arm64" ;;
        *) error "Unsupported architecture: $arch"; exit 1 ;;
    esac
    
    echo "forgy-${os}-${arch}"
}

# Get latest release version
get_latest_version() {
    curl -s "https://api.github.com/repos/${REPO}/releases/latest" | \
        grep '"tag_name":' | \
        sed -E 's/.*"([^"]+)".*/\1/'
}

# Download and install forgy
install_forgy() {
    local platform version url temp_file
    
    platform=$(detect_platform)
    version=$(get_latest_version)
    
    if [ -z "$version" ]; then
        error "Failed to get latest version"
        exit 1
    fi
    
    info "Installing forgy $version for $platform"
    
    url="https://github.com/${REPO}/releases/download/${version}/${platform}"
    temp_file=$(mktemp)
    
    info "Downloading from $url"
    if ! curl -fsSL "$url" -o "$temp_file"; then
        error "Failed to download forgy"
        exit 1
    fi
    
    # Create install directory
    mkdir -p "$INSTALL_DIR"
    
    # Install binary
    chmod +x "$temp_file"
    mv "$temp_file" "$INSTALL_DIR/forgy"
    
    info "forgy installed to $INSTALL_DIR/forgy"
    
    # Check if install directory is in PATH
    if ! echo "$PATH" | grep -q "$INSTALL_DIR"; then
        warn "Add $INSTALL_DIR to your PATH to use forgy from anywhere:"
        echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
        echo "  echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> ~/.bashrc"
    fi
    
    info "Installation complete! Run 'forgy --help' to get started."
}

# Main execution
main() {
    install_forgy
}

main "$@"