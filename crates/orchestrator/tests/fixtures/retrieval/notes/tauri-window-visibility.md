# Tauri WebviewWindow.isFocused lies during animation

Calling `WebviewWindow.isFocused()` inside the macOS minimize /
restore animation window returns the *destination* focus state,
not the current one. This makes the inbox-surface-notification
gating fire a beat early. Read `document.visibilityState`
instead — it transitions only after the animation frame commits.
