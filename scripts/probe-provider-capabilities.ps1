[CmdletBinding()]
param(
    [string]$SettingsPath = (Join-Path $env:APPDATA "ClaudeScienceAssistant\settings.json"),
    [ValidateRange(256, 65536)]
    [int]$LargeOutputProbe = 32768,
    [ValidateRange(5, 120)]
    [int]$TimeoutSeconds = 45
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Security
Add-Type -AssemblyName System.Net.Http

function Unprotect-ApiKey {
    param([Parameter(Mandatory)][string]$Encrypted)

    $cipher = [Convert]::FromBase64String($Encrypted)
    $plain = [Security.Cryptography.ProtectedData]::Unprotect(
        $cipher,
        $null,
        [Security.Cryptography.DataProtectionScope]::CurrentUser
    )
    [Text.Encoding]::UTF8.GetString($plain)
}

function Invoke-ProviderJson {
    param(
        [Parameter(Mandatory)][System.Net.Http.HttpClient]$Client,
        [Parameter(Mandatory)][ValidateSet("Get", "Post")][string]$Method,
        [Parameter(Mandatory)][string]$Url,
        [Parameter(Mandatory)][string]$ApiKey,
        [AllowNull()]$Body
    )

    $request = [System.Net.Http.HttpRequestMessage]::new(
        [System.Net.Http.HttpMethod]::$Method,
        $Url
    )
    $request.Headers.Authorization = [System.Net.Http.Headers.AuthenticationHeaderValue]::new(
        "Bearer",
        $ApiKey
    )
    if ($null -ne $Body) {
        $json = $Body | ConvertTo-Json -Depth 20 -Compress
        $request.Content = [System.Net.Http.StringContent]::new(
            $json,
            [Text.Encoding]::UTF8,
            "application/json"
        )
    }

    try {
        $response = $Client.SendAsync($request).GetAwaiter().GetResult()
        $raw = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
        $parsed = $null
        try {
            $parsed = $raw | ConvertFrom-Json
        }
        catch {
            # A non-JSON error page is still represented by its status and size.
        }
        [pscustomobject]@{
            Status = [int]$response.StatusCode
            Json = $parsed
            RawLength = $raw.Length
        }
    }
    catch {
        [pscustomobject]@{
            Status = 0
            Json = $null
            RawLength = 0
            TransportError = $_.Exception.GetType().Name
        }
    }
    finally {
        $request.Dispose()
    }
}

if (-not (Test-Path -LiteralPath $SettingsPath -PathType Leaf)) {
    throw "CSA settings file was not found."
}

$settings = Get-Content -LiteralPath $SettingsPath -Raw -Encoding utf8 | ConvertFrom-Json
$entries = @($settings.apiKeys) |
    Where-Object { $_.encryptedApiKey -and $_.baseUrl -and $_.model } |
    Group-Object { "$($_.providerId)|$($_.baseUrl)|$($_.model)" } |
    ForEach-Object { $_.Group[-1] }

$client = [System.Net.Http.HttpClient]::new()
$client.Timeout = [TimeSpan]::FromSeconds($TimeoutSeconds)
$results = @()

try {
    foreach ($entry in $entries) {
        $configuredBaseUrl = ([string]$entry.baseUrl).TrimEnd("/")
        $baseUrl = if ($configuredBaseUrl.EndsWith("/v1") -or $configuredBaseUrl.EndsWith("/v4")) {
            $configuredBaseUrl
        }
        else {
            "$configuredBaseUrl/v1"
        }
        $model = [string]$entry.model
        $apiKey = Unprotect-ApiKey -Encrypted ([string]$entry.encryptedApiKey)

        try {
            $models = Invoke-ProviderJson -Client $client -Method Get `
                -Url "$baseUrl/models" -ApiKey $apiKey -Body $null
            $modelItems = if ($models.Json -and $models.Json.data) { @($models.Json.data) } else { @() }
            $modelIds = @($modelItems | ForEach-Object { [string]$_.id })
            $selectedMetadata = @($modelItems | Where-Object { [string]$_.id -eq $model }) | Select-Object -First 1

            $text = Invoke-ProviderJson -Client $client -Method Post `
                -Url "$baseUrl/chat/completions" -ApiKey $apiKey -Body @{
                    model = $model
                    messages = @(@{ role = "user"; content = "Reply with exactly OK." })
                    max_tokens = 256
                    stream = $false
                    parallel_tool_calls = $true
                }
            $textMessage = if ($text.Json -and $text.Json.choices) {
                $text.Json.choices[0].message
            }
            else { $null }
            $visibleText = if ($textMessage) { [string]$textMessage.content } else { "" }
            $reasoningText = if ($textMessage) { [string]$textMessage.reasoning_content } else { "" }
            $textFinish = if ($text.Json -and $text.Json.choices) {
                [string]$text.Json.choices[0].finish_reason
            }
            else { "" }

            $large = Invoke-ProviderJson -Client $client -Method Post `
                -Url "$baseUrl/chat/completions" -ApiKey $apiKey -Body @{
                    model = $model
                    messages = @(@{ role = "user"; content = "Reply with exactly OK." })
                    max_tokens = $LargeOutputProbe
                    stream = $false
                }
            $largeFinish = if ($large.Json -and $large.Json.choices) {
                [string]$large.Json.choices[0].finish_reason
            }
            else { "" }

            $tool = Invoke-ProviderJson -Client $client -Method Post `
                -Url "$baseUrl/chat/completions" -ApiKey $apiKey -Body @{
                    model = $model
                    messages = @(@{
                        role = "user"
                        content = "Use the add_numbers function for 2 and 3. Do not answer directly."
                    })
                    max_tokens = 256
                    stream = $false
                    tools = @(@{
                        type = "function"
                        function = @{
                            name = "add_numbers"
                            description = "Add two integers"
                            parameters = @{
                                type = "object"
                                properties = @{
                                    a = @{ type = "integer" }
                                    b = @{ type = "integer" }
                                }
                                required = @("a", "b")
                            }
                        }
                    })
                    tool_choice = @{
                        type = "function"
                        function = @{ name = "add_numbers" }
                    }
                }
            $toolMessage = if ($tool.Json -and $tool.Json.choices) {
                $tool.Json.choices[0].message
            }
            else { $null }
            [array]$toolCalls = if ($toolMessage -and $toolMessage.tool_calls) {
                @($toolMessage.tool_calls)
            }
            else { @() }
            $toolFinish = if ($tool.Json -and $tool.Json.choices) {
                [string]$tool.Json.choices[0].finish_reason
            }
            else { "" }

            $reasoningBody = @{
                model = $model
                messages = @(@{ role = "user"; content = "Reply with exactly OK." })
                max_tokens = 256
                stream = $false
            }
            $modelLower = $model.ToLowerInvariant()
            if ($modelLower -match "glm|kimi|moonshot|deepseek|mimo") {
                $reasoningBody.thinking = @{ type = "enabled" }
            }
            elseif ($modelLower -match "qwen") {
                $reasoningBody.enable_thinking = $true
            }
            elseif ($modelLower -match "minimax") {
                $reasoningBody.reasoning_split = $true
            }
            elseif ($modelLower -match "(^|/)o[0-9]|(^|/)gpt-([5-9]|[1-9][0-9])") {
                $reasoningBody.reasoning_effort = "high"
            }
            $reasoning = if ($reasoningBody.Count -gt 4) {
                Invoke-ProviderJson -Client $client -Method Post `
                    -Url "$baseUrl/chat/completions" -ApiKey $apiKey -Body $reasoningBody
            }
            else { $null }
            $reasoningMessage = if ($reasoning -and $reasoning.Json -and $reasoning.Json.choices) {
                $reasoning.Json.choices[0].message
            }
            else { $null }

            $errorClass = ""
            if ($models.Status -eq 401 -or $text.Status -eq 401) {
                $errorClass = "auth"
            }
            elseif ($text.Status -eq 404) {
                $errorClass = "route-or-model"
            }
            elseif ($text.Status -ge 400 -or $text.Status -eq 0) {
                $errorClass = "request-or-transport"
            }

            $results += [pscustomobject]@{
                provider = [string]$entry.providerId
                label = [string]$entry.label
                baseUrl = $configuredBaseUrl
                model = $model
                modelsStatus = $models.Status
                modelCount = $modelIds.Count
                selectedListed = $modelIds -contains $model
                metadataHasOutputLimit = [bool](
                    $selectedMetadata -and (
                        $null -ne $selectedMetadata.max_output_tokens -or
                        $null -ne $selectedMetadata.max_tokens -or
                        $null -ne $selectedMetadata.context_window
                    )
                )
                textStatus = $text.Status
                visibleText = -not [string]::IsNullOrWhiteSpace($visibleText)
                reasoningOnly = (
                    [string]::IsNullOrWhiteSpace($visibleText) -and
                    -not [string]::IsNullOrWhiteSpace($reasoningText)
                )
                textFinish = $textFinish
                largeProbeTokens = $LargeOutputProbe
                largeStatus = $large.Status
                acceptsLargeProbe = $large.Status -ge 200 -and $large.Status -lt 300
                largeFinish = $largeFinish
                toolStatus = $tool.Status
                toolCallCount = $toolCalls.Count
                nativeToolCall = $toolCalls.Count -gt 0
                toolMessageFields = if ($toolMessage) {
                    @($toolMessage.PSObject.Properties.Name | Sort-Object)
                }
                else { @() }
                embeddedToolMarker = if ($toolMessage) {
                    ([string]$toolMessage.content) -match "tool_call|function"
                }
                else { $false }
                toolFinish = $toolFinish
                reasoningProbeStatus = if ($reasoning) { $reasoning.Status } else { $null }
                reasoningControlAccepted = if ($reasoning) {
                    $reasoning.Status -ge 200 -and $reasoning.Status -lt 300
                }
                else { $null }
                reasoningContentReturned = if ($reasoningMessage) {
                    -not [string]::IsNullOrWhiteSpace([string]$reasoningMessage.reasoning_content)
                }
                else { $false }
                errorClass = $errorClass
            }
        }
        finally {
            $apiKey = $null
        }
    }
}
finally {
    $client.Dispose()
}

$results | ConvertTo-Json -Depth 6
