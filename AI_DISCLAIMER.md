# AI disclaimer

Much of Bifrost was written by an AI coding assistant: Anthropic's Claude,
working through Claude Code. That includes most of the Rust and Python code,
the tests, the documentation, and the patches in Bifrost's Archipelago fork.

A human maintainer directed the work. They decided what to build and how it
should behave, made the design and security decisions, and reviewed and tested
the results. The code also went through automated and AI-assisted reviews for
bugs and security issues, and those findings were fixed.

What that means for you:

- **It can still be wrong.** AI-written code can contain subtle bugs or
  security flaws that review and testing missed. Treat Bifrost like any other
  young, small open-source project.
- **You run it at your own risk.** Bifrost is provided as is, without
  warranty (see [LICENSE](LICENSE)). Before exposing an instance to the
  internet, read the [user guide](docs/USER_GUIDE.md)'s notes on isolation and
  sign-in, and keep backups.
- **Apworlds are code.** Custom Archipelago worlds run arbitrary Python.
  Bifrost sandboxes them, but no sandbox is perfect. Only allow worlds you
  trust.
- **Reports are welcome.** If you find a bug, please open an issue. For a
  security problem, open an issue without the details and ask for a private
  channel.
