# Renders docs/changes.gif: two scans of the demo home with growth between
# them, opened with `spacehog --changes`. Run from the repository root.

Output docs/changes.gif
Source docs/demo-setup.tape

Hide
Type@1ms "scripts/demo-tree.sh $HOME grow && $SH scan $HOME >/dev/null && clear"
Enter
Sleep 3s
Type@1ms "$SH --changes"
Enter
Wait+Screen /Changes/
Show
Sleep 3s
Down
Sleep 500ms
Down
Sleep 500ms
Up
Sleep 500ms
Up
Sleep 800ms
Type "["
Sleep 2s
Type "1"
Sleep 2s
Type "6"
Sleep 1.5s
Type "q"
Type "q"
