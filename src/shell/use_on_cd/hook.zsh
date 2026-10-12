_rnm_use_on_cd() {
  command rnm use --from-hook
  rehash
}
autoload -Uz add-zsh-hook
add-zsh-hook -D chpwd _rnm_use_on_cd
add-zsh-hook chpwd _rnm_use_on_cd
rehash
