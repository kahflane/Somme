# Somme shell hooks. They print marks (OSC 6973) so Somme can show each
# command as a block:
#   A;<exit code>;<folder>   a prompt is about to show
#   B                        the user accepted a line
#   C;<command>              the command starts running
#   P;<path>                 PATH changed, for completing command names
#   H;<folder>               the home folder, for showing ~ on a server
# Text is percent-encoded so it cannot end a mark early. Each mark starts with
# a secret Somme picked for this shell, so program output cannot fake one.

builtin unset SOMME_SHELL_HOOKS
[[ -n "${_somme_loaded-}" ]] && builtin return 0
typeset -g _somme_loaded=1

_somme_encode() {
  builtin local s=$1
  s=${s//\%/%25}
  s=${s//$'\n'/%0A}
  s=${s//$'\r'/%0D}
  s=${s//$'\a'/%07}
  s=${s//$'\e'/%1B}
  s=${s//$'\x18'/%18}
  s=${s//$'\x1a'/%1A}
  typeset -g REPLY=$s
}

_somme_mark() {
  builtin printf '\e]6973;%s;%s\a' "$_somme_token" "$1"
}

_somme_precmd() {
  builtin local code=$?
  if [[ $HOME != "${_somme_home-}" ]]; then
    typeset -g _somme_home=$HOME
    _somme_encode "$HOME"
    _somme_mark "H;$REPLY"
  fi
  if [[ $PATH != "${_somme_path-}" ]]; then
    typeset -g _somme_path=$PATH
    _somme_encode "$PATH"
    _somme_mark "P;$REPLY"
  fi
  _somme_encode "$PWD"
  _somme_mark "A;$code;$REPLY"
}

# zsh marks an unfinished last line with a % before each prompt. The prompt
# is hidden, so that mark would stay in the block. Themes turn it back on in
# their own precmd, so turn it off again after every line.
_somme_line_finish() {
  builtin unsetopt prompt_sp
  _somme_mark B
}

_somme_preexec() {
  builtin unsetopt prompt_sp
  _somme_encode "$1"
  _somme_mark "C;$REPLY"
}

# Runs at the first prompt, after the user's config, so the prompt hook runs
# first (it needs the exit code) and the command hook runs last.
_somme_init() {
  precmd_functions=(_somme_precmd ${precmd_functions:#(_somme_init|_somme_precmd)})
  preexec_functions=(${preexec_functions:#_somme_preexec} _somme_preexec)
  builtin autoload -Uz add-zle-hook-widget && add-zle-hook-widget line-finish _somme_line_finish
  builtin unsetopt prompt_sp
  _somme_precmd
}

precmd_functions+=(_somme_init)
