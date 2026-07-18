# Contributing to MagnumDB

First off, thank you for considering contributing to MagnumDB! We actively encourage contributions from developers of all skill levels, especially beginners. Our goal is to make database development accessible.

## Getting Started

1. **Fork & Clone:** Fork the repository on GitHub and clone your fork locally.
2. **Setup:** Ensure you have the latest stable Rust installed (`rustup update`).
3. **Build:** Run `cargo build` to compile the project.
4. **Test:** Run `cargo test` to execute the test suite.

## Finding Something to Work On

- Check the GitHub Issues page.
- Look for issues labeled `good first issue` or `beginner`.
- Feel free to ask questions on any issue if you need guidance.

## Development Workflow

1. **Create a Branch:** `git checkout -b feature/your-feature-name`
2. **Write Code:** Implement your feature or bug fix.
   - **Quality:** Ensure you run `cargo clippy -- -D warnings` and `cargo fmt`.
   - **Testing:** Add unit or integration tests for your changes.
   - **Documentation:** Document new public functions and explain complex logic using comments. We value well-commented code for educational purposes.
3. **Commit:** Write clear, concise commit messages.
4. **Push:** Push to your fork and submit a Pull Request against the `main` branch.

## Code Standards

- Follow Rust best practices.
- Avoid `unsafe` code unless it is thoroughly justified, documented, and reviewed.
- Use `anyhow` for application-level error handling.
- Use `log` and `env_logger` for structured logging.

## Need Help?

Reach out to the maintainers on our Discord or via GitHub Discussions. We are happy to help you navigate the codebase!
