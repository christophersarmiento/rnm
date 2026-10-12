function __rnm_find_version_file
    set -l dir $PWD
    while true
        for name in .nvmrc .node-version
            if test -f "$dir/$name"
                echo "$dir/$name"
                return
            end
        end
        test -n "$dir"; or return
        set dir (string replace -r '/[^/]*$' '' -- $dir)
    end
end
function __rnm_use_on_cd --on-variable PWD
    status --is-command-substitution; and return
    set -l found (__rnm_find_version_file)
    if test "$found" != "$__rnm_version_file"
        set -g __rnm_version_file $found
        command rnm use --from-hook
    end
end
set -g __rnm_version_file (__rnm_find_version_file)
