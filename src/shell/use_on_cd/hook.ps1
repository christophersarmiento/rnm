function global:__rnm_use_on_cd {
  if ($PWD.Provider.Name -ne 'FileSystem') { return }
  $exitCode = $global:LASTEXITCODE
  # Out-Host: an event handler's output is discarded, and prompt's output
  # becomes the prompt.
  & rnm use --from-hook | Out-Host
  $global:LASTEXITCODE = $exitCode
}
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
    $global:__rnm_pwd = $PWD.ProviderPath
    $global:__rnm_previous_prompt = $function:prompt
    function global:prompt {
      if ($PWD.ProviderPath -ne $global:__rnm_pwd) {
        $global:__rnm_pwd = $PWD.ProviderPath
        __rnm_use_on_cd
      }
      & $global:__rnm_previous_prompt
    }
  }
}
