# Contributing

Thanks for looking. OpenAs is small on purpose; changes that keep it that way
are the easiest to take.

## Building

    ./install.sh          # build everything and install OpenAs in ~/Applications

To work without touching your own apps, give the build a separate setup:

    OPENAS_ROOT=/tmp/oa OPENAS_INSTALL_DIR=/tmp/oa/apps OPENAS_ID_PREFIX=app.openas.dev. \
      target/release/openas setup

## Before a pull request

    cargo fmt --all
    cargo clippy --all-targets -- -D warnings
    cargo test

CI runs the same three on macOS.

## Where things are

- The builder's instructions: `core/prompts/builder.md`.
- What an app's page can call: `runner/src/api.rs` and `runner/src/powers/`.
- What the Helper accepts: the `Request` enum in `cli/src/helper.rs`.

Anything that widens what an app may do (a new power, a new Helper request)
needs a note in SECURITY.md in the same pull request.
