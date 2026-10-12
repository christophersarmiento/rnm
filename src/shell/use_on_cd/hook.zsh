_rnm_find_version_file() {
  _rnm_found=
  local dir=$PWD
  while :; do
    if [ -f "$dir/.nvmrc" ]; then
      _rnm_found=$dir/.nvmrc
      return
    fi
    if [ -f "$dir/.node-version" ]; then
      _rnm_found=$dir/.node-version
      return
    fi
    [ -n "$dir" ] || return
    dir=${dir%/*}
  done
}
_rnm_use_on_cd() {
  _rnm_find_version_file
  if [ "$_rnm_found" != "${_rnm_version_file-}" ]; then
    _rnm_version_file=$_rnm_found
    command rnm use --from-hook
    rehash
  fi
}
autoload -Uz add-zsh-hook
add-zsh-hook -D chpwd _rnm_use_on_cd
add-zsh-hook chpwd _rnm_use_on_cd
_rnm_find_version_file
_rnm_version_file=$_rnm_found
rehash
