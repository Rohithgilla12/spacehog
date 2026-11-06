# Renders docs/cleanup.gif: queue items, run them to the (demo) trash, then
# approve and run an agent's proposal that arrives over MCP mid-recording.
# Run from the repository root: vhs docs/cleanup.tape

Output docs/cleanup.gif
Source docs/demo-setup.tape

Type@1ms "(sleep 16; scripts/agent-propose.sh 'npm and Homebrew caches refill on the next install; old-dashboard has been idle for 8 months.' $HOME/.npm/_cacache $HOME/Library/Caches/Homebrew $HOME/code/old-dashboard/node_modules >/dev/null 2>&1) & $SH"
Enter
Wait+Screen /Overview/
Show
Sleep 1.5s
Down
Sleep 400ms
Type "d"
Sleep 600ms
Down
Sleep 400ms
Down
Sleep 400ms
Type "d"
Sleep 800ms
Type "5"
Sleep 3s
Type "D"
Sleep 3.5s
Enter
Sleep 2s
Right
Sleep 3s
Enter
Sleep 2s
Type "D"
Sleep 3.5s
Type "u"
Sleep 2.5s
Enter
Sleep 1s
Type "q"
Type "q"
