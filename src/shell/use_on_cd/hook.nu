let __rnm_find_version_file = {|start|
  mut dir = $start
  loop {
    for name in ['.nvmrc' '.node-version'] {
      let file = ($dir | path join $name)
      if ($file | path type) == 'file' { return $file }
    }
    let parent = ($dir | path dirname)
    if $parent == $dir { return '' }
    $dir = $parent
  }
}
$env.__RNM_VERSION_FILE = (do $__rnm_find_version_file $env.PWD)
let __rnm_other_pwd_hooks = (
  $env.config?.hooks?.env_change?.PWD?
  | default []
  | where {|hook| not (($hook | describe | str starts-with 'record') and ('__rnm_use_on_cd' in ($hook | columns))) }
)
$env.config = ($env.config? | default {} | upsert hooks.env_change.PWD ($__rnm_other_pwd_hooks | append {
  __rnm_use_on_cd: true
  code: {|_, after|
    let found = (do $__rnm_find_version_file $after)
    let changed = $found != ($env.__RNM_VERSION_FILE? | default '')
    $env.__RNM_VERSION_FILE = $found
    if $changed { try { ^rnm use --from-hook } }
  }
}))
