# Contributing to TriOS

First off, thank you for considering contributing to **TriOS**! Projects like this thrive because of curious developers, kernel hackers, and contributors.

## How Can I Contribute?

### Reporting Bugs
If you find a bug, crash, or unexpected behavior while running TriOS:
1. Check the [Issues](https://github.com/jojin1709/trios/issues) tab to see if it's already reported.
2. If not, open a new Issue with:
   - Your environment (Hypervisor, e.g. VirtualBox 7.x, QEMU, or Bare-Metal hardware)
   - Steps to reproduce
   - Exact serial log output (`serial.log`) or hypervisor error messages.

### Suggesting Enhancements
Have an idea for a driver, new shell app, or protocol? Feel free to open a feature request in Issues describing the design and use case!

### Code Guidelines
- Written in **Rust** targeting `x86_64-unknown-none` (no standard library, `#![no_std]`).
- Clean separation of concerns (hardware drivers in `kernel/src/drivers/`, protocols in `kernel/src/net/`, GUI apps in `kernel/src/apps/`).
- Code must compile with **0 compiler warnings and 0 errors** (`cargo build --release`).
- Keep code safe wherever possible, document `unsafe` blocks with clear `// SAFETY:` invariant comments.

### Pull Requests
1. Fork the repo and create your feature branch: `git checkout -b feature/my-new-driver`.
2. Commit your changes: `git commit -am 'feat: add support for XYZ'`.
3. Push to the branch: `git push origin feature/my-new-driver`.
4. Open a Pull Request on GitHub.

## Code of Conduct

This project adheres to the [Contributor Covenant](CODE_OF_CONDUCT.md). By participating, you are expected to uphold this code.

## Maintainer & Contact

**TriOS** is maintained and created by **JOJIN JOHN** ([@jojin1709](https://github.com/jojin1709)).
Connect on LinkedIn: [linkedin.com/in/jojin-john](https://www.linkedin.com/in/jojin-john/)
