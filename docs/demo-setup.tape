# Shared by the docs/*.tape recordings: builds the synthetic demo home from
# scripts/demo-tree.sh, points HOME, the index, and the trash at target/demo,
# and scans it. Nothing outside target/demo is read or cleaned.

Set Shell "bash"
Set FontSize 15
Set Width 1440
Set Height 860
Set Padding 14
Set Theme "Dracula"
Set TypingSpeed 60ms
Set WaitTimeout 300s

Hide
Type@1ms "cargo build -q -p spacehog && export SH=$PWD/target/debug/spacehog SPACEHOG_BIN=$PWD/target/debug/spacehog HOME=$PWD/target/demo/home SPACEHOG_DATA_DIR=$PWD/target/demo/index SPACEHOG_STATE_DIR=$PWD/target/demo/state SPACEHOG_CONFIG_DIR=$PWD/target/demo/config SPACEHOG_TRASH_DIR=$PWD/target/demo/trash COLORTERM=truecolor PS1='$ ' && rm -rf target/demo && scripts/demo-tree.sh $HOME && $SH scan $HOME >/dev/null && echo ready-$((40+2))"
Enter
Wait+Screen /ready-42/
Type@1ms "clear"
Enter
Sleep 300ms
