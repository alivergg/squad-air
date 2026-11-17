# SquadAir Optional Notification Sound Cues

Place standard 16-bit or 32-bit float `.wav` files (48,000 Hz recommended) in this directory (`./assets/sounds/`) or in your OS user configuration directory (`squad-air/sounds/`).

If any file is missing, `squad-air` silently skips playback for that event without errors or warnings.

| Filename | Trigger Event |
| :--- | :--- |
| `join.wav` | Local user joins a voice room |
| `leave.wav` | Local user leaves a voice room |
| `peer_join.wav` | Another participant joins the room |
| `peer_leave.wav` | Another participant leaves or times out |
| `mute.wav` | Microphone is muted |
| `unmute.wav` | Microphone is unmuted |