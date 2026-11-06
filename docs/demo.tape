# Renders docs/demo.gif, the 60-second tour: Overview, Explorer, Treemap,
# then an agent's proposal arriving over MCP, its approval, and the cleanup.
# Everything runs against the synthetic home from scripts/demo-tree.sh.
# Run from the repository root: vhs docs/demo.tape

Output docs/demo.gif
Source docs/demo-setup.tape

Type@1ms "(sleep 30; AGENT_NAME=claude-code scripts/agent-propose.sh 'These are regenerable build outputs and caches; old-dashboard has been idle for 8 months.' $HOME/code/old-dashboard/node_modules $HOME/code/api-rs/target $HOME/.npm/_cacache $HOME/Library/Caches/Homebrew >/dev/null 2>&1) & $SH"
Enter
Wait+Screen /Overview/
Show
Sleep 5s
Right
Sleep 1.2s
Right
Sleep 1.2s
Left
Left
Sleep 700ms
Down
Sleep 400ms
Down
Sleep 400ms
Up
Up
Sleep 500ms
Enter
Sleep 1.8s
Down
Sleep 400ms
Type "p"
Sleep 2s
Left
Sleep 800ms
Right
Sleep 1.5s
Type "3"
Sleep 1.5s
Type "+"
Sleep 1.5s
Enter
Sleep 2s
Enter
Sleep 2s
Backspace
Sleep 1s
Backspace
Sleep 1.5s
Type "4"
Sleep 2s
Down
Sleep 500ms
Enter
Sleep 3s
Type "1"
Sleep 3s
Type "5"
Sleep 1.5s
Right
Sleep 3.5s
Enter
Sleep 2s
Type "D"
Sleep 7s
Enter
Sleep 1s
Type "1"
Sleep 6s
Type "q"
Type "q"
