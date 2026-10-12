__rnm_find_version_file() {
  __rnm_found=
  local dir=$PWD
  while :; do
    if [ -f "$dir/.nvmrc" ]; then
      __rnm_found=$dir/.nvmrc
      return
    fi
    if [ -f "$dir/.node-version" ]; then
      __rnm_found=$dir/.node-version
      return
    fi
    [ -n "$dir" ] || return
    dir=${dir%/*}
  done
}
__rnm_use_on_cd() {
  local status=$?
  __rnm_find_version_file
  if [ "$__rnm_found" != "${__rnm_version_file-}" ]; then
    __rnm_version_file=$__rnm_found
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
__rnm_find_version_file
__rnm_version_file=$__rnm_found
if [[ ";${PROMPT_COMMAND[*]:-};" != *";__rnm_use_on_cd;"* ]]; then
  if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
    PROMPT_COMMAND=(__rnm_use_on_cd "${PROMPT_COMMAND[@]}")
  else
    PROMPT_COMMAND="__rnm_use_on_cd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
  fi
fi
