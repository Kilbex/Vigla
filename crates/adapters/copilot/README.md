# GitHub Copilot adapter

Line-oriented fallback for GitHub Copilot CLI output. The CLI can emit JSON, but
Vigla does not claim structured support until captured fixtures and an
end-to-end gate prove that contract.

```sh
cargo test -p vigla-adapter-copilot
```

Keep process spawning and authentication checks outside this crate.
