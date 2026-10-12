let __rnm_other_pwd_hooks = (
  $env.config?.hooks?.env_change?.PWD?
  | default []
  | where {|hook| not (($hook | describe | str starts-with 'record') and ('__rnm_use_on_cd' in ($hook | columns))) }
)
$env.config = ($env.config? | default {} | upsert hooks.env_change.PWD ($__rnm_other_pwd_hooks | append {
  __rnm_use_on_cd: true
  code: {|before, after| try { ^rnm use --from-hook } }
}))
