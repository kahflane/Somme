# Somme's hooks for bash. A bash on a server starts with this file in place
# of ~/.bashrc: it loads the user's login files, as `bash -l` would, then
# prints the same marks as the zsh hooks (see ../zsh/somme.zsh).

# The marks' secret stays in an unexported variable, so no program started
# from this shell can read it and print fake marks.
_somme_token=${SOMME_MARK_TOKEN-}
builtin unset SOMME_MARK_TOKEN

# This file's folder was made for this shell alone, and bash has read the
# whole file by now.
if [[ ${SOMME_HOOKS_TMP-} == */somme.* && -d $SOMME_HOOKS_TMP ]]; then
  command rm -rf -- "$SOMME_HOOKS_TMP"
fi
builtin unset SOMME_HOOKS_TMP

if [[ -r /etc/profile ]]; then
  builtin source /etc/profile
fi
for _somme_file in ~/.bash_profile ~/.bash_login ~/.profile; do
  if [[ -r $_somme_file ]]; then
    builtin source "$_somme_file"
    break
  fi
done
builtin unset _somme_file

_somme_encode() {
  local s=$1
  s=${s//\%/%25}
  s=${s//$'\n'/%0A}
  s=${s//$'\r'/%0D}
  s=${s//$'\a'/%07}
  s=${s//$'\e'/%1B}
  s=${s//$'\x18'/%18}
  s=${s//$'\x1a'/%1A}
  REPLY=$s
}

_somme_mark() {
  builtin printf '\033]6973;%s;%s\007' "$_somme_token" "$1"
}

# Runs first of the prompt commands, while $? still holds the command's code.
_somme_prompt() {
  local code=$?
  _somme_ready=
  if [[ $HOME != "${_somme_home-}" ]]; then
    _somme_home=$HOME
    _somme_encode "$HOME"
    _somme_mark "H;$REPLY"
  fi
  if [[ $PATH != "${_somme_path-}" ]]; then
    _somme_path=$PATH
    _somme_encode "$PATH"
    _somme_mark "P;$REPLY"
  fi
  _somme_encode "$PWD"
  _somme_mark "A;$code;$REPLY"
  return $code
}

# Runs last of the prompt commands, so the next thing the DEBUG trap sees is
# the user's command.
_somme_prompt_done() {
  local code=$?
  _somme_ready=1
  return $code
}

_somme_command() {
  _somme_encode "$1"
  _somme_mark B
  _somme_mark "C;$REPLY"
}

# bash has no hook for "a command starts", so the DEBUG trap stands in. It
# fires before every simple command; only the first one after a prompt counts.
# Tab completion and key bindings run commands too, and do not count. An
# empty line goes straight back to the prompt commands.
_somme_preexec() {
  [[ -n ${_somme_ready-} && -z ${COMP_LINE-} && -z ${READLINE_POINT-} ]] || return 0
  [[ $BASH_COMMAND == _somme_prompt* ]] && return 0
  _somme_ready=
  local line command=
  line=$(HISTTIMEFORMAT= builtin history 1)
  # The same last entry means this line stayed out of the history.
  if [[ $line != "${_somme_last-}" ]]; then
    _somme_last=$line
    command=${line#*[[:digit:]][* ] }
  fi
  _somme_command "$command"
}

_somme_status() {
  return "$1"
}

_somme_last=$(HISTTIMEFORMAT= builtin history 1)
if [[ -n ${bash_preexec_imported-}${__bp_imported-} ]]; then
  # bash-preexec already owns the DEBUG trap, and runs hooks for us.
  precmd_functions=(_somme_prompt "${precmd_functions[@]}")
  preexec_functions+=(_somme_command)
else
  if [[ $(builtin declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a"* ]]; then
    PROMPT_COMMAND=(_somme_prompt "${PROMPT_COMMAND[@]}" _somme_prompt_done)
  else
    PROMPT_COMMAND=$'_somme_prompt\n'${PROMPT_COMMAND-}$'\n_somme_prompt_done'
  fi
  # Keep a DEBUG trap the user already has, after ours. This stays out of
  # functions: bash hides the DEBUG trap inside them.
  _somme_debug=$(builtin trap -p DEBUG)
  if [[ -n $_somme_debug ]]; then
    _somme_capture() { _somme_debug=$2; }
    builtin eval "_somme_capture ${_somme_debug#trap }"
    builtin unset -f _somme_capture
    builtin trap '_somme_code=$?; _somme_preexec; _somme_status "$_somme_code"; builtin eval -- "$_somme_debug"' DEBUG
  else
    builtin trap _somme_preexec DEBUG
  fi
fi
