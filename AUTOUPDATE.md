# Auto-Update System for Canopy

## Overview

Canopy now includes an automatic update system that checks for new stable releases on GitHub and updates the binary when a new version is available.

## Features

- **Automatic Update Checks**: Checks for updates once every 24 hours
- **Stable Only**: Only updates to stable releases (format X.X.X, not X.X.X-text)
- **Platform Detection**: Automatically detects your OS and architecture
- **Atomic Updates**: Downloads and verifies updates before replacing the current binary
- **Non-Intrusive**: Runs in the background without interrupting your workflow

## How It Works

1. **Update Check**: When Canopy starts, it checks if 24 hours have passed since the last update check
2. **GitHub API**: Fetches the latest releases from the GitHub repository
3. **Version Comparison**: Compares the current version with the latest stable release
4. **Download**: If a newer stable version is available, downloads the appropriate binary for your platform
5. **Installation**: Replaces the current binary with the new version
6. **Notification**: Shows a brief message indicating the update was successful

## Configuration

The auto-update system is enabled by default. It stores the last update check time in:

```
~/.canopy/last_update_check.txt
```

## Disabling Auto-Updates

If you want to disable auto-updates, you can:

1. Remove the `last_update_check.txt` file to prevent future checks
2. The system will respect this and not perform automatic updates

## Manual Updates

You can always manually update by:

1. Downloading the latest release from GitHub: https://github.com/UniverLab/agent-canopy/releases
2. Using the install script: `curl -fsSL https://raw.githubusercontent.com/UniverLab/agent-canopy/main/scripts/install.sh | sh`

## Technical Details

### Version Comparison

The system uses semantic version comparison:
- Compares major, minor, and patch versions
- Only updates when the new version is strictly greater than the current version
- Ignores pre-release versions (beta, rc, alpha, etc.)

### Platform Support

Currently supports:
- **Linux**: x86_64, aarch64 (musl libc)
- **macOS**: x86_64, aarch64

### Update Process

1. Detects current platform (OS + architecture)
2. Constructs the appropriate download URL
3. Downloads the .tar.gz archive
4. Extracts the canopy binary
5. Atomically replaces the current binary
6. Cleans up temporary files

## Safety

- Uses temporary files for downloads
- Verifies file integrity before replacement
- Atomic file replacement to prevent corruption
- Graceful error handling for network issues

## Troubleshooting

If auto-updates aren't working:

1. Check your internet connection
2. Verify GitHub API accessibility
3. Check the logs in `~/.canopy/daemon.log`
4. Try manually updating using the install script

## Implementation

The auto-update system is implemented in `src/autoupdate.rs` and includes:

- `should_check_for_updates()`: Determines if an update check should be performed
- `check_for_updates()`: Fetches and compares versions from GitHub
- `perform_autoupdate()`: Downloads and installs the update
- `check_and_update_if_needed()`: Main entry point called during startup

The system is integrated into the main application flow in `src/main.rs`.