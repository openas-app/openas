# Security

OpenAs runs code that a model wrote. This page says what that code can and
can't reach, and where the limits are.

## What an app can do

Every app is a copy of one window program (the Runner) showing the app's own
folder of HTML, CSS and JavaScript in the system's WebKit. It has three levels:

1. **What a web page can do (the default).** The internet over https, the
   camera, microphone and screen (macOS asks first), and files the person picks
   or drops in. Code is served only from the app's own folder: a content
   security policy blocks scripts from anywhere else.
2. **App permissions, asked when the app needs them:** notifications, a menu
   bar item, staying on top, global keyboard shortcuts, and one folder the
   person chooses. File calls are confined to that folder in Rust: paths are
   relative, `..` is refused, and every path is resolved through symlinks and
   must still be inside it.
3. **Full access:** running programs and reading or changing any file. Asked
   in its own warning, only when the app asks for it.

Permissions are stored next to the app's code, never inside it, so the model
can't grant them. They belong to one version of the app: after any change, the
app asks again.

## Shared apps

A `.openas` file holds an app's code and icon, never its data. Opening one
checks it before anything is installed: no links, only the expected files,
`index.html` required, and limits on size and file count. A shared app starts
with no permissions and can never get full access, even after you change it.
You can read its code before saving it.

## The Helper

Making and changing apps goes through the Helper, a background process with a
Unix socket only your user can open. It checks who is calling, from the
calling process's id, its program and that program's bundle, and answers only:

- the `openas` command itself;
- the OpenAs window it installed (checked by its code signature hash);
- an app it installed, checked the same way, and only about that app (change,
  undo, versions and restore).

`cli/tests/helper_refuses.rs` starts a real Helper and checks that a stranger
and a modified copy of an app are refused.

This keeps apps from acting on each other. It is not protection from malware
already running as your user, which could do anything you can.

## Keys and spending

Provider keys are kept in the macOS Keychain and sent only to the provider you
chose. Nothing goes to a server of ours; there isn't one. Each build has a
spending limit per app and in total.

## Reporting a problem

Please report security problems privately through GitHub's "Report a
vulnerability" on this repository, not in a public issue.
