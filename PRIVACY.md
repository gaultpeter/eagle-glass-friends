# Privacy

Eagle Glass is a desktop client. It contains no telemetry, no analytics, no crash
reporting and no update check. Nothing is sent to the author.

This is the **friends build**, which adds an optional "who is watching" presence
feature. It is off unless you configure a presence URL and token, and it is
described separately below.

## What is stored locally

On first run the app creates `%APPDATA%\EagleGlass\config.txt`, containing:

- the Xtream panel server URL,
- the panel username,
- the panel password, in plaintext,
- the preferred live format (`ts` or `m3u8`),
- the display name you chose,
- a randomly generated `device_id`,
- the presence worker URL and shared token, if configured.

The file is plaintext by design, so treat it like a credential — it holds your
panel password and the presence token. Use **Reset app…** in the settings panel
to delete it and return to the first-run wizard; that also clears your presence
entry so others stop seeing a watcher that no longer exists.

## What leaves the machine

The app makes requests only to the Xtream panel you configure: login,
categories, channel lists, EPG, and the video streams themselves. Those requests
go directly from your machine to your provider. They do not pass through, and
are not visible to, the author of this application.

VLC is loaded from your own local installation of VLC (`libvlc.dll`). This
application does not download, bundle or update it.

## Presence data

When a presence URL and token are configured, the app sends these fields to that
worker (a Cloudflare Worker that you or your group operate):

- the display name you entered in the wizard,
- the randomly generated `device_id`,
- the name of the channel currently playing,
- whether playback is running or paused, and timestamps.

It does **not** send your panel username, your panel password, or any part of the
stream. A heartbeat is sent every 90 seconds while a stream is up, and roster
reads occur while the app runs. Entries expire on the worker roughly three
minutes after the last heartbeat.

The shared token is compiled into the binary and is not a secret; it authenticates
the group, not the individual.

Leave the presence URL empty to disable the feature entirely. The app then sends
nothing anywhere except to your panel.

## Contact

Open an issue at <https://github.com/gaultpeter/eagle-glass-friends/issues>.
