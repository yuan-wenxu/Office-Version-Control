param([switch] $Uninstall)

$ErrorActionPreference = 'Stop'
$commandDirectory = Join-Path $PSScriptRoot 'bin'
$commandPath = Join-Path $commandDirectory 'ovc.exe'
$environment = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey(
    'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', $true
)

try {
    $rawPath = $environment.GetValue(
        'Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames
    )
    $pathKind = [Microsoft.Win32.RegistryValueKind]::ExpandString
    if ($environment.GetValueNames() -contains 'Path') {
        $pathKind = $environment.GetValueKind('Path')
    }
    $entries = @($rawPath -split ';' | Where-Object {
        [Environment]::ExpandEnvironmentVariables($_.Trim().Trim('"')).TrimEnd('\', '/') -ine $commandDirectory
    })

    if ($Uninstall) {
        $newPath = $entries -join ';'
    } else {
        $source = Join-Path $PSScriptRoot 'ovc-cli.exe'
        New-Item -ItemType Directory -Path $commandDirectory -Force | Out-Null
        Copy-Item -LiteralPath $source -Destination $commandPath -Force
        $newPath = (($entries + $commandDirectory) -join ';').TrimStart(';')
    }

    if ($newPath -cne $rawPath) {
        $environment.SetValue('Path', $newPath, $pathKind)
    }

    if ($Uninstall) {
        if (Test-Path -LiteralPath $commandPath) {
            Remove-Item -LiteralPath $commandPath
        }
        if ((Test-Path -LiteralPath $commandDirectory) -and
            -not (Get-ChildItem -LiteralPath $commandDirectory -Force | Select-Object -First 1)) {
            Remove-Item -LiteralPath $commandDirectory
        }
    }
} finally {
    $environment.Dispose()
}
