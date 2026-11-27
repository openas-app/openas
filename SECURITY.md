# Security

OpenAs runs code that a model wrote. This page says what that code can and
can't reach, and where the limits are.

## What an app can do

Every app is a copy of one window program (the Runner) showing the app's own
folder of HTML, CSS and JavaScript in the system's WebKit. It has three levels:

1. **An offline web page (the default).** The camera, microphone and screen
   (macOS asks first), and files the person picks or drops in. The app's files
   are served only from its own folder, and a content security policy allows
   nothing from anywhere else: no requests, images, fonts or frames from the
   web. Links to the web don't open, and WebRTC peer connections and link
   preconnects are off (the policy doesn't cover WebRTC's STUN step, and a
   preconnect looks up any host name). WebKit's DNS prefetching is off by
   default.
2. **App permissions, asked when the app needs them:** the internet (https and
   wss), notifications, a menu bar item, staying on top, global keyboard
   shortcuts, and one folder the person chooses. File calls are confined to
   that folder in Rust: paths are relative, `..` is refused, and every path is
   resolved through symlinks and must still be inside it. With the internet
   allowed, an app can download code and run it (`eval`), as any web page
   online can.
3. **Full access:** running programs and reading or changing any file, which
   includes asking the Keychain for your provider key (see below). Asked in its
   own warning, only when the app asks for it.

Permissions are stored next to the app's code, never inside it, so the model
can't grant them. They belong to one version of the app: after any change, the
app asks again. A change you're trying out before saving it counts as its own
version, so it asks again too; a yes given to it carries over when that same
code is saved. An app that had the internet asks again by itself after a change,
whether or not its code asks.

The internet limit is a content security policy in WebKit, not a firewall, and
an app with full access can run programs that ignore it. While you try a new app
in OpenAs before saving it, it gets the internet if it asks; the installed app
asks you at its first launch. Apps made before the internet became a permission
keep it for the version they run, until they change.

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

## Keys, data and spending

Provider keys are kept in the macOS Keychain and sent only to the provider you
chose. Nothing goes to a server of ours; there isn't one. OpenAs stores keys
through Apple's `security` tool, so any program running as you, including an
app you gave full access, can read them without a prompt. While a key is being
saved it is briefly visible in the process list.

What goes to the model provider: your description, the app's code, and, while
building or changing, screenshots and test results of the app running in a
simulated desktop. When you change an app, that test run uses the app's saved
data, so parts of it can appear in what the model sees.

The builder's simulated desktop serves only the app's own files and uses the
same content security policy as the real app. It is offline unless the app
being changed had the internet or its code asks for it (in a check, the person
says yes).

Each build has a spending limit per app and in total.

## Reporting a problem

Please report security problems privately through GitHub's "Report a
vulnerability" on this repository, not in a public issue.
