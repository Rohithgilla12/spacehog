# Renders docs/overview.gif: Overview, then Explorer, then the info panel.
# Run from the repository root with `vhs docs/overview.tape`. The tape scans
# this repository into target/demo-index first, so it never touches $HOME.

Output docs/overview.gif

Set Shell "bash"
Set FontSize 14
Set Width 1440
Set Height 820
Set Padding 16
Set Theme "Dracula"
Set TypingSpeed 60ms
Set WaitTimeout 300s

Env SPACEHOG_DATA_DIR "target/demo-index"
Env SPACEHOG_CONFIG_DIR "target/demo-index"
Env SPACEHOG_STATE_DIR "target/demo-index"
Env COLORTERM "truecolor"

Hide
Type "cargo build -q -p spacehog && rm -rf target/demo-index && ./target/debug/spacehog scan . >/dev/null && echo index built: $((40+2))"
Enter
Wait+Screen /index built: 42/
Type "clear"
Enter
Show

Type "./target/debug/spacehog"
Enter
Sleep 2.5s

Right
Sleep 1.2s
Right
Sleep 1.2s
Right
Sleep 1.2s
Left
Left
Left
Sleep 800ms
Down
Sleep 400ms
Down
Sleep 400ms
Up
Up
Sleep 600ms

Enter
Sleep 2s
Down
Sleep 400ms
Down
Sleep 400ms
Up
Sleep 400ms
Type "p"
Sleep 2s
Type "i"
Sleep 3.5s
Escape
Sleep 600ms
Left
Sleep 800ms
Left
Sleep 800ms
Type "?"
Sleep 2s
Escape
Type "1"
Sleep 1s
Type "q"
Sleep 300ms
