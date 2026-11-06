# Renders docs/overview.gif: Overview, then Explorer, then the info panel.
# Run from the repository root: vhs docs/overview.tape

Output docs/overview.gif
Source docs/demo-setup.tape

Type@1ms "$SH"
Enter
Wait+Screen /Overview/
Show
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
