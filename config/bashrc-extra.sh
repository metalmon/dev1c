# bash additions for the 1C working environment.
# Sourced from /etc/bash.bashrc, applies to every user.
#
# The file lives in /etc/profile.d, so /etc/profile walks it with sh as well —
# and `shopt` below then prints "shopt: not found" into every login, the RDP
# session's startup log included. Nothing here means anything outside bash, so
# leave at once when that is not the shell.
[ -n "${BASH_VERSION-}" ] || return 0

# --- History ----------------------------------------------------------------
# By default history is lost with parallel sessions and gets truncated. A
# developer over RDP usually keeps several terminals open — write from all of
# them.
HISTSIZE=50000
HISTFILESIZE=50000
HISTCONTROL=ignoreboth:erasedups
HISTTIMEFORMAT='%d.%m %H:%M  '
shopt -s histappend cmdhist checkwinsize

# --- Prompt -----------------------------------------------------------------
# Coloured, with the current git branch: when working on a configuration dump
# the branch is the thing people most often forget to check before editing.
__git_branch() {
    git rev-parse --abbrev-ref HEAD 2>/dev/null | sed 's/^/ (/;s/$/)/'
}
PS1='\[\e[1;36m\]\u\[\e[0m\]:\[\e[1;34m\]\w\[\e[0;33m\]$(__git_branch)\[\e[0m\]\$ '

# --- Aliases ----------------------------------------------------------------
alias ll='ls -lh --group-directories-first --color=auto'
alias la='ls -lha --group-directories-first --color=auto'
alias ls='ls --color=auto'
alias grep='grep --color=auto'
alias ..='cd ..'
alias ...='cd ../..'

# Debian installs fd as fdfind — restore the familiar name
command -v fdfind >/dev/null && alias fd='fdfind'

# Git: the commands typed a hundred times a day
alias gs='git status -sb'
alias gd='git diff'
alias gl='git log --oneline --graph --decorate -20'

# Searching a dump: ripgrep aware that 1C sources are UTF-8 XML and BSL
alias rgbsl='rg --type-add "bsl:*.bsl" --type-add "x1c:*.xml" -t bsl -t x1c'

# --- Conveniences -----------------------------------------------------------
export EDITOR=nano
export PAGER=less
export LESS='-R -F -X'
