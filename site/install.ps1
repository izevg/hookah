# hookah installer for Windows (PowerShell 5.1 and PowerShell 7+).
#
#   irm https://izevg.github.io/hookah/install.ps1 | iex
#
# Environment:
#   HOOKAH_VERSION          install this release instead of the latest
#   HOOKAH_INSTALL_DIR      install here (default: %LOCALAPPDATA%\hookah\bin)
#   HOOKAH_NO_MODIFY_PATH   set to 1 to leave the user PATH alone
#   HOOKAH_DOWNLOAD_BASE    where release assets live (default: GitHub release downloads)
#   HOOKAH_API_URL          the releases API base (the last two exist for the installer tests)
#
# It downloads the zip and SHA256SUMS, verifies the checksum, installs hookah.exe, and adds the
# install directory to your user PATH. No administrator rights needed.

function Install-Hookah {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'

    $repo = 'izevg/hookah'
    $downloadBase = if ($env:HOOKAH_DOWNLOAD_BASE) { $env:HOOKAH_DOWNLOAD_BASE } else { "https://github.com/$repo/releases/download" }
    $apiUrl = if ($env:HOOKAH_API_URL) { $env:HOOKAH_API_URL } else { "https://api.github.com/repos/$repo/releases" }

    # Windows PowerShell 5.1 does not negotiate TLS 1.2 by default.
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch {
        Write-Verbose 'could not set TLS 1.2; continuing'
    }

    # PROCESSOR_ARCHITEW6432 is set when a 32-bit PowerShell runs on a 64-bit system.
    $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    switch ($arch) {
        'AMD64' { $target = 'x86_64-pc-windows-msvc' }
        'ARM64' { $target = 'aarch64-pc-windows-msvc' }
        default { throw "hookah-install: unsupported CPU architecture: '$arch' (supported: AMD64, ARM64)" }
    }

    $version = $env:HOOKAH_VERSION
    if ($version) {
        $version = $version.TrimStart('v')
    } else {
        $tag = $null
        try { $tag = (Invoke-RestMethod -UseBasicParsing -Uri "$apiUrl/latest").tag_name } catch { $tag = $null }
        if (-not $tag) {
            # No stable release yet: fall back to the newest release of any kind.
            try { $tag = (Invoke-RestMethod -UseBasicParsing -Uri "${apiUrl}?per_page=1" | Select-Object -First 1).tag_name } catch { $tag = $null }
            if ($tag) { Write-Host "hookah-install: no stable release found, using the newest prerelease: $tag" }
        }
        if (-not $tag) {
            throw "hookah-install: could not find the latest release at $apiUrl (set `$env:HOOKAH_VERSION)"
        }
        $version = $tag.TrimStart('v')
    }
    if ($version -notmatch '^[0-9][0-9A-Za-z.+-]*$') {
        throw "hookah-install: invalid version: $version"
    }

    $installDir = if ($env:HOOKAH_INSTALL_DIR) { $env:HOOKAH_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'hookah\bin' }
    $name = "hookah-$version-$target"
    $archive = "$name.zip"
    $url = "$downloadBase/v$version/$archive"
    $sumsUrl = "$downloadBase/v$version/SHA256SUMS"

    Write-Host "hookah-install: version: $version"
    Write-Host "hookah-install: target:  $target"
    Write-Host "hookah-install: install: $installDir\hookah.exe"

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("hookah-install-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $zipPath = Join-Path $tmp $archive
        $sumsPath = Join-Path $tmp 'SHA256SUMS'
        Write-Host "hookah-install: downloading $archive"
        try {
            Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $zipPath
            Invoke-WebRequest -UseBasicParsing -Uri $sumsUrl -OutFile $sumsPath
        } catch {
            throw "hookah-install: download failed: $($_.Exception.Message)"
        }

        $expected = $null
        foreach ($line in Get-Content -Path $sumsPath) {
            if ($line -match '^([0-9a-fA-F]{64})\s+\*?(.+?)\s*$' -and $Matches[2] -eq $archive) {
                $expected = $Matches[1].ToLowerInvariant()
                break
            }
        }
        if (-not $expected) { throw "hookah-install: SHA256SUMS has no entry for $archive" }
        $actual = (Get-FileHash -Algorithm SHA256 -Path $zipPath).Hash.ToLowerInvariant()
        if ($actual -ne $expected) {
            throw "hookah-install: checksum mismatch for ${archive}: expected $expected, got $actual (nothing installed)"
        }
        Write-Host 'hookah-install: checksum ok'

        Expand-Archive -Path $zipPath -DestinationPath $tmp -Force
        $exe = Join-Path $tmp "$name\hookah.exe"
        if (-not (Test-Path $exe)) { throw "hookah-install: the archive does not contain $name\hookah.exe" }

        New-Item -ItemType Directory -Path $installDir -Force | Out-Null
        Copy-Item -Path $exe -Destination (Join-Path $installDir 'hookah.exe') -Force
        Write-Host "hookah-install: installed hookah $version to $installDir\hookah.exe"
    } finally {
        Remove-Item -Recurse -Force -Path $tmp -ErrorAction SilentlyContinue
    }

    if ($env:HOOKAH_NO_MODIFY_PATH -ne '1') {
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $entries = @()
        if ($userPath) { $entries = $userPath -split ';' | Where-Object { $_ } }
        $wanted = $installDir.TrimEnd('\')
        $present = $entries | Where-Object { $_.TrimEnd('\') -ieq $wanted }
        if (-not $present) {
            $newPath = (@($entries) + $installDir) -join ';'
            [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
            Write-Host "hookah-install: added $installDir to your user PATH. Restart your terminal to pick it up."
        }
        # Make hookah usable in this session too.
        if (-not (($env:Path -split ';') | Where-Object { $_.TrimEnd('\') -ieq $wanted })) {
            $env:Path = "$env:Path;$installDir"
        }
    }

    Write-Host 'hookah-install: next steps:'
    Write-Host '  hookah setup --detect      # which agent harnesses are installed'
    Write-Host '  hookah setup <harness>     # claude, codex, cursor, qoder, kimi, opencode, hermes, pi, or openclaw'
}

Install-Hookah
