#!/usr/bin/env sh
set -eu

cargo test runtime::native::tests::captures_xeyes_frame -- --ignored
cargo test runtime::native::tests::injects_pointer_and_keyboard_events -- --ignored
cargo test runtime::native::tests::observes_application_crash_and_cleans_up -- --ignored
cargo test runtime::native::tests::reports_xvfb_crash -- --ignored

if command -v xmessage >/dev/null 2>&1; then
    cargo test runtime::native::tests::smoke_tests_xmessage -- --ignored
fi

echo "Set MICRO_GUI_GTK_SMOKE=1 to opt into the environment-dependent Mousepad test."
