function global:__rnm_find_version_file {
  $dir = $PWD.ProviderPath
  while ($dir) {
    foreach ($name in '.nvmrc', '.node-version') {
      $file = [IO.Path]::Combine($dir, $name)
      if ([IO.File]::Exists($file)) { return $file }
    }
    $dir = [IO.Path]::GetDirectoryName($dir)
  }
  return ''
}
function global:__rnm_use_on_cd {
  if ($PWD.Provider.Name -ne 'FileSystem') { return }
  $found = __rnm_find_version_file
  if ($found -ne $global:__rnm_version_file) {
    $global:__rnm_version_file = $found
    $exitCode = $global:LASTEXITCODE
    # Out-Host: an event handler's output is discarded, and prompt's output
    # becomes the prompt.
    & rnm use --from-hook | Out-Host
    $global:LASTEXITCODE = $exitCode
  }
}
$global:__rnm_version_file = __rnm_find_version_file
if (-not (Test-Path variable:global:__rnm_use_on_cd_registered)) {
  $global:__rnm_use_on_cd_registered = $true
  if ($PSVersionTable.PSVersion -ge [version]'6.1') {
    $handler = [EventHandler[System.Management.Automation.LocationChangedEventArgs]] { __rnm_use_on_cd }
    $previous = $ExecutionContext.SessionState.InvokeCommand.LocationChangedAction
    $ExecutionContext.SessionState.InvokeCommand.LocationChangedAction = if ($previous) {
      [Delegate]::Combine($previous, $handler)
    } else {
      $handler
    }
  } else {
    $global:__rnm_previous_prompt = $function:prompt
    function global:prompt {
      __rnm_use_on_cd
      & $global:__rnm_previous_prompt
    }
  }
}
