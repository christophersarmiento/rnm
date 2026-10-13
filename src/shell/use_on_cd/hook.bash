__rnm_use_on_cd() {
  local status=$?
  if [ "$PWD" != "${__rnm_pwd-}" ]; then
    __rnm_pwd=$PWD
    command rnm use --from-hook
    hash -r
  fi
  return $status
}
# `\cd` skips the alias but still runs a user's own `cd` function.
__rnm_cd() {
  \cd "$@" || return
  __rnm_use_on_cd
}
alias cd=__rnm_cd
__rnm_pwd=$PWD
if [[ ";${PROMPT_COMMAND[*]:-};" != *";__rnm_use_on_cd;"* ]]; then
  if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
    PROMPT_COMMAND=(__rnm_use_on_cd "${PROMPT_COMMAND[@]}")
  else
    PROMPT_COMMAND="__rnm_use_on_cd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
  fi
fi
