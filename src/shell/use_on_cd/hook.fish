function __rnm_use_on_cd --on-variable PWD
    status --is-command-substitution; and return
    command rnm use --from-hook
end
