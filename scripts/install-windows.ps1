# Perkament installer - NinjaOne/SYSTEM and interactive Windows PowerShell 5.1+
#
# Scope AllUsers  : installs to Program Files (needs SYSTEM or an elevated prompt).
#                   Users cannot replace the program there, so the in-app updater
#                   stays quiet; update by running this script again.
# Scope CurrentUser: installs to the user's profile; the in-app updater keeps it current.
[CmdletBinding()]
param(
    [ValidateSet('Auto','AllUsers','CurrentUser')]
    [string]$Scope = 'Auto',
    # Release tag such as 'v0.1.0-build.12'. Empty = the latest release.
    [string]$Version = '',
    # Pin the download to a hash you obtained from a trusted place (SHA256SUMS.txt of the release).
    [string]$ExpectedSha256 = '',
    # Path to minisign.exe. When given (or found on PATH) the release signature is verified.
    [string]$MinisignPath = '',
    # Stop when the signature cannot be verified (no minisign, or an unsigned release).
    [switch]$RequireSignature,
    [string]$IconPath = '',
    [switch]$NoDesktopShortcut
)

$ErrorActionPreference = 'Stop'
# Windows PowerShell 5.1 does not always offer TLS 1.2 by default; GitHub requires it.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$asset = 'Perkament-Windows.zip'
# Public key the releases are signed with (same key the in-app updater trusts).
$publicKey = 'RWR7s/OSI0i7+yEDK54DPU4NLXF9PkX7AIOvE53+V2UhSKGw6iFBfxr7'
if ($Version) {
    if ($Version -notmatch '^v[0-9A-Za-z.\-]+$') { throw "Ongeldige versie: $Version" }
    $releaseUrl = "https://github.com/PatrickUdo/perkament/releases/download/$Version/$asset"
} else {
    $releaseUrl = "https://github.com/PatrickUdo/perkament/releases/latest/download/$asset"
}

function Require-Path([string]$Value, [string]$Label) {
    if ([string]::IsNullOrWhiteSpace($Value)) { throw "Kan $Label niet bepalen (leeg Windows-profielpad)." }
    return $Value
}

function New-PerkamentIcon {
    param([Parameter(Mandatory)][string]$Destination)

    # Make a self-contained, recognizable app icon (parchment sheet with a P).
    # Windows PowerShell 5.1 on Windows includes System.Drawing.
    Add-Type -AssemblyName System.Drawing
    $bitmap = [System.Drawing.Bitmap]::new(256, 256)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $graphics.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    $paperBrush = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(255, 244, 218, 163))
    $edgePen = [System.Drawing.Pen]::new([System.Drawing.Color]::FromArgb(255, 117, 78, 41), 9)
    $inkBrush = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(255, 91, 52, 28))
    $scrollBrush = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(255, 221, 177, 110))
    $font = [System.Drawing.Font]::new('Georgia', 135, [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
    $pngStream = [System.IO.MemoryStream]::new()
    try {
        $graphics.Clear([System.Drawing.Color]::Transparent)
        $graphics.FillRectangle($paperBrush, 36, 26, 184, 204)
        $graphics.DrawRectangle($edgePen, 36, 26, 184, 204)
        $graphics.FillEllipse($scrollBrush, 23, 208, 210, 30)
        $graphics.DrawEllipse($edgePen, 23, 208, 210, 30)
        $graphics.DrawString('P', $font, $inkBrush, 77, 44)
        $bitmap.Save($pngStream, [System.Drawing.Imaging.ImageFormat]::Png)
        $pngBytes = $pngStream.ToArray()
        # ICO directory header + one 256x256 PNG image; Windows supports PNG-in-ICO.
        $fileStream = [System.IO.File]::Create($Destination)
        $writer = [System.IO.BinaryWriter]::new($fileStream)
        try {
            $writer.Write([uint16]0)                 # Reserved
            $writer.Write([uint16]1)                 # Icon type
            $writer.Write([uint16]1)                 # Image count
            $writer.Write([byte]0)                   # Width 256
            $writer.Write([byte]0)                   # Height 256
            $writer.Write([byte]0)                   # Palette size
            $writer.Write([byte]0)                   # Reserved
            $writer.Write([uint16]1)                 # Planes
            $writer.Write([uint16]32)                # Bits/pixel
            $writer.Write([uint32]$pngBytes.Length)  # PNG length
            $writer.Write([uint32]22)                # PNG offset
            $writer.Write($pngBytes)
        } finally {
            $writer.Dispose()
        }
    } finally {
        $pngStream.Dispose()
        $font.Dispose()
        $scrollBrush.Dispose()
        $inkBrush.Dispose()
        $edgePen.Dispose()
        $paperBrush.Dispose()
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

function New-AppShortcut {
    param([Parameter(Mandatory=$true)][string]$ShortcutPath,
          [Parameter(Mandatory=$true)][string]$Target,
          [Parameter(Mandatory=$true)][string]$Icon)
    $shortcutFolder = Split-Path -Parent $ShortcutPath
    New-Item -ItemType Directory -Path $shortcutFolder -Force | Out-Null
    $shell = New-Object -ComObject WScript.Shell
    $link = $shell.CreateShortcut($ShortcutPath)
    $link.TargetPath = $Target
    $link.WorkingDirectory = Split-Path -Parent $Target
    $link.IconLocation = "$Icon,0"
    $link.Description = 'Perkament'
    $link.Save()
}

# Removes the files this installer used to put in ProgramData. Standard users can
# create files in a ProgramData subfolder, so a program for all users does not belong there.
function Remove-LegacyInstall {
    $legacy = Join-Path (Require-Path $env:ProgramData 'ProgramData') 'Perkament'
    if (-not (Test-Path -LiteralPath $legacy)) { return }
    foreach ($name in 'perkament.exe', 'perkament.ico') {
        $file = Join-Path $legacy $name
        if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file -Force -ErrorAction SilentlyContinue }
    }
    # Only remove the folder when nothing else is in it.
    if (-not (Get-ChildItem -LiteralPath $legacy -Force -ErrorAction SilentlyContinue)) {
        Remove-Item -LiteralPath $legacy -Force -ErrorAction SilentlyContinue
        Write-Host "Oude installatie verwijderd: $legacy"
    } else {
        Write-Warning "Oude map bevat nog andere bestanden en blijft staan: $legacy"
    }
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$isSystem = $identity.User.Value -eq 'S-1-5-18'
$isAdmin = ([Security.Principal.WindowsPrincipal]$identity).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if ($Scope -eq 'Auto') {
    if ($isSystem) { $Scope = 'AllUsers' } else { $Scope = 'CurrentUser' }
}
if ($Scope -eq 'CurrentUser' -and $isSystem) {
    throw 'CurrentUser onder SYSTEM zou Perkament in het SYSTEM-profiel installeren. Gebruik -Scope AllUsers.'
}
if ($Scope -eq 'AllUsers' -and -not ($isSystem -or $isAdmin)) {
    throw 'AllUsers installeert in Program Files en heeft beheerdersrechten nodig. Start PowerShell als administrator of gebruik -Scope CurrentUser.'
}

if ($Scope -eq 'AllUsers') {
    $programFiles = Require-Path $env:ProgramFiles 'ProgramFiles'
    $base = Require-Path $env:ProgramData 'ProgramData'
    $public = Require-Path $env:PUBLIC 'PUBLIC'
    $installDir = Join-Path $programFiles 'Perkament'
    $menuDir = Join-Path $base 'Microsoft\Windows\Start Menu\Programs'
    $desktopDir = Join-Path $public 'Desktop'
} else {
    $local = Require-Path $env:LOCALAPPDATA 'LOCALAPPDATA'
    $roaming = Require-Path $env:APPDATA 'APPDATA'
    $desktopDir = Require-Path ([Environment]::GetFolderPath('Desktop')) 'Desktop'
    $installDir = Join-Path $local 'Programs\Perkament'
    $menuDir = Join-Path $roaming 'Microsoft\Windows\Start Menu\Programs'
}
$exePath = Join-Path $installDir 'perkament.exe'
$installedIconPath = Join-Path $installDir 'perkament.ico'
$startMenuShortcut = Join-Path $menuDir 'Perkament.lnk'
$desktopShortcut = Join-Path $desktopDir 'Perkament.lnk'
$tempDir = Join-Path ([IO.Path]::GetTempPath()) ('perkament-install-' + [guid]::NewGuid().ToString('N'))
$zipPath = Join-Path $tempDir $asset
$extractDir = Join-Path $tempDir 'unpacked'

try {
    Write-Host "Installatiemodus: $Scope; identiteit: $($identity.Name)"
    New-Item -ItemType Directory -Path $tempDir,$extractDir -Force | Out-Null
    Write-Host "Downloaden: $releaseUrl"
    Invoke-WebRequest -Uri $releaseUrl -OutFile $zipPath -UseBasicParsing
    $hash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash
    Write-Host "SHA-256 download: $hash"
    if ($ExpectedSha256) {
        if (($ExpectedSha256.Length -ne 64) -or ($ExpectedSha256 -match '[^a-fA-F0-9]')) { throw 'ExpectedSha256 moet 64 hexadecimale tekens bevatten.' }
        if ($hash -ne $ExpectedSha256.ToUpperInvariant()) { throw 'SHA-256 komt niet overeen. Installatie gestopt.' }
    }

    # Signature check with minisign, when it is available.
    $minisign = $MinisignPath
    if (-not $minisign) {
        $found = Get-Command minisign -ErrorAction SilentlyContinue
        if ($found) { $minisign = $found.Source }
    }
    $verified = $false
    if ($minisign) {
        $signaturePath = "$zipPath.minisig"
        try {
            Invoke-WebRequest -Uri "$releaseUrl.minisig" -OutFile $signaturePath -UseBasicParsing
        } catch {
            if ($RequireSignature) { throw 'Deze release heeft geen handtekening. Installatie gestopt.' }
            Write-Warning 'Deze release heeft geen handtekening.'
        }
        if (Test-Path -LiteralPath $signaturePath) {
            & $minisign -V -P $publicKey -m $zipPath -x $signaturePath | Out-Null
            if ($LASTEXITCODE -ne 0) { throw 'De handtekening van de download klopt niet. Installatie gestopt.' }
            $verified = $true
            Write-Host 'Handtekening gecontroleerd.'
        }
    } elseif ($RequireSignature) {
        throw 'RequireSignature is gezet, maar minisign is niet gevonden. Geef -MinisignPath op.'
    }
    if (-not $verified -and -not $ExpectedSha256) {
        Write-Warning 'Download niet gecontroleerd: geef -ExpectedSha256 mee of installeer minisign voor de handtekeningcontrole.'
    }

    Expand-Archive -LiteralPath $zipPath -DestinationPath $extractDir -Force
    $candidates = @(Get-ChildItem -LiteralPath $extractDir -Filter 'perkament.exe' -File -Recurse)
    if ($candidates.Count -ne 1) { throw "Verwacht 1 perkament.exe; gevonden: $($candidates.Count)." }

    New-Item -ItemType Directory -Path $installDir,$menuDir -Force | Out-Null
    if ($IconPath) {
        $resolved = (Resolve-Path -LiteralPath $IconPath -ErrorAction Stop).ProviderPath
        if ([IO.Path]::GetExtension($resolved) -ine '.ico') { throw 'IconPath moet naar een .ico-bestand verwijzen.' }
        if ($resolved -ine $installedIconPath) { Copy-Item -LiteralPath $resolved -Destination $installedIconPath -Force }
    } else {
        New-PerkamentIcon -Destination $installedIconPath
    }

    # A running program cannot be overwritten, but it can be renamed. Move the old one
    # aside first, so the install also works while Perkament is open.
    Get-ChildItem -LiteralPath $installDir -Filter '.perkament-old-*' -Force -ErrorAction SilentlyContinue |
        Remove-Item -Force -ErrorAction SilentlyContinue
    $oldPath = $null
    if (Test-Path -LiteralPath $exePath) {
        $oldPath = Join-Path $installDir ('.perkament-old-' + [guid]::NewGuid().ToString('N'))
        Move-Item -LiteralPath $exePath -Destination $oldPath -Force
    }
    try {
        Copy-Item -LiteralPath $candidates[0].FullName -Destination $exePath -Force
    } catch {
        # Put the previous version back when the new one could not be placed.
        if ($oldPath -and -not (Test-Path -LiteralPath $exePath)) { Move-Item -LiteralPath $oldPath -Destination $exePath -Force }
        throw
    }
    if ($oldPath) { Remove-Item -LiteralPath $oldPath -Force -ErrorAction SilentlyContinue }
    Unblock-File -LiteralPath $exePath

    New-AppShortcut -ShortcutPath $startMenuShortcut -Target $exePath -Icon $installedIconPath
    if (-not $NoDesktopShortcut) {
        New-Item -ItemType Directory -Path $desktopDir -Force | Out-Null
        New-AppShortcut -ShortcutPath $desktopShortcut -Target $exePath -Icon $installedIconPath
    }
    if ($Scope -eq 'AllUsers') { Remove-LegacyInstall }

    Write-Host "Perkament geinstalleerd: $exePath"
    Write-Host "Startmenu: $startMenuShortcut"
    if (-not $NoDesktopShortcut) { Write-Host "Bureaublad: $desktopShortcut" }
    if ($Scope -eq 'AllUsers') {
        Write-Host 'Updates: voer dit script opnieuw uit; de app werkt zichzelf hier niet bij.'
    } else {
        Write-Host 'Updates: de app controleert zelf op nieuwe versies.'
    }
    Write-Host 'Let op: taakbalk vastmaken is een aparte Windows-gebruikersactie.'
} finally {
    if (Test-Path -LiteralPath $tempDir) {
        Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
