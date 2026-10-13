# Helm 整包快照 —— 一条命令备份「源码 + 完整 git 历史 + 本地独有文件」
#
# 用法(本机为 Windows PowerShell 5.1,用 powershell 不是 pwsh):
#   powershell -ExecutionPolicy Bypass -File scripts\snapshot.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\snapshot.ps1 -Keep 20
#   powershell -ExecutionPolicy Bypass -File scripts\snapshot.ps1 -Tag p126-done
#   powershell -ExecutionPolicy Bypass -File scripts\snapshot.ps1 -List
#
# 产出(F:\Helm-backups\snapshot-<时间戳>[-<标签>]\):
#   helm-repo.bundle   完整 git 历史单文件(git clone 可直接还原)
#   src\               工作区源码(排除 node_modules/target/dist/logs/dataset/.git)
#   AGENTS.md          项目档案(已 gitignore,不入库,丢了最难重建)
#   config.yaml        运行配置(含 DPAPI 密文,仅本机可解)
#   known_hosts.json   主机密钥 TOFU 记录
#   _MANIFEST.txt      HEAD/同步状态/测试计数,便于回溯"这是哪一版"
#
# 为什么 bundle + 源码双份:源码便于直接翻看/复制单个文件,
# bundle 保证历史与分支完整可还原(源码目录没有 .git)。
# 注意:bundle 与 src 都含内网 IP/凭据线索 —— 本目录**绝不可入库或外传**。
#
# 编码:本文件必须存为 **UTF-8 with BOM**。PowerShell 5.1 读无 BOM 的 .ps1
# 会按 GBK 解码,中文注释变乱码并直接导致语法错误(实测踩过)。

[CmdletBinding()]
param(
  [int]$Keep = 10,
  [string]$Tag = "",
  [switch]$List
)

$ErrorActionPreference = 'Stop'
$Repo = 'F:\Helm'
$BackupRoot = 'F:\Helm-backups'

function Write-Step($msg) { Write-Host "  $msg" }

# 原生命令(git/robocopy)会把正常信息写 stderr;PS 5.1 在 ErrorActionPreference=Stop
# 下会把它当致命错误抛出(NativeCommandError)。故统一经此包装:
# 临时降级为 Continue,按退出码判定成败。
function Invoke-Native {
  param([scriptblock]$Cmd)
  $old = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  try {
    $out = & $Cmd 2>&1 | Out-String
    return [pscustomobject]@{ Out = $out; Code = $LASTEXITCODE }
  } finally {
    $ErrorActionPreference = $old
  }
}

# 注意:函数名不能叫 Git —— PowerShell 函数名不区分大小写,`& git` 会递归调用
# 自己直到 CallDepthOverflow。故用 Invoke-Git。
# 坑:$Args 是 PowerShell 自动变量,在脚本块内会被遮蔽成空数组 →
# git 收到零参数只打印帮助。故用具名参数 $GitArgs 显式传递。
function Invoke-Git {
  param([Parameter(ValueFromRemainingArguments = $true)][string[]]$GitArgs)
  $repo = $Repo
  return Invoke-Native { & git -C $repo @GitArgs }
}

if (-not (Test-Path $BackupRoot)) { New-Item -ItemType Directory -Path $BackupRoot -Force | Out-Null }

# ---------- -List:列出现有快照 ----------
if ($List) {
  $all = @(Get-ChildItem $BackupRoot -Directory -Filter 'snapshot-*' | Sort-Object Name -Descending)
  if ($all.Count -eq 0) { Write-Host "尚无快照"; exit 0 }
  Write-Host "现有快照 $($all.Count) 份(新→旧):"
  foreach ($d in $all) {
    $sz = [math]::Round(((Get-ChildItem $d.FullName -Recurse -File -ErrorAction SilentlyContinue |
      Measure-Object -Property Length -Sum).Sum) / 1MB, 1)
    $head = ""
    $mf = Join-Path $d.FullName '_MANIFEST.txt'
    if (Test-Path $mf) {
      $line = (Get-Content $mf -Encoding UTF8 | Where-Object { $_ -like 'git HEAD*' } | Select-Object -First 1)
      if ($line) { $head = ($line -replace '^git HEAD\s*:\s*', '') }
    }
    Write-Host ("  {0,-34} {1,6} MB  {2}" -f $d.Name, $sz, $head)
  }
  exit 0
}

# ---------- 建快照 ----------
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$name = if ($Tag) { "snapshot-$stamp-$Tag" } else { "snapshot-$stamp" }
$dst = Join-Path $BackupRoot $name
New-Item -ItemType Directory -Path $dst -Force | Out-Null
Write-Host "快照 → $dst"

# 1) git bundle:完整历史(含所有分支/tag)
Write-Step "1/5 git bundle(完整历史)…"
$bundle = Join-Path $dst 'helm-repo.bundle'
$r = Invoke-Git bundle create $bundle --all
if ($r.Code -ne 0) { throw "bundle 创建失败: $($r.Out)" }
if (-not (Test-Path $bundle)) { throw "bundle 未生成" }
$v = Invoke-Git bundle verify $bundle
if ($v.Out -notmatch 'complete history') { throw "bundle 校验未通过: $($v.Out)" }
Write-Step "    OK 校验通过(complete history)"

# 2) 工作区源码(排除依赖/产物)
Write-Step "2/5 复制源码树…"
$srcDst = Join-Path $dst 'src'
New-Item -ItemType Directory -Path $srcDst -Force | Out-Null
$excl = @('node_modules', '.git', 'dist', 'target', 'logs', 'dataset', '.svelte-kit', '.trae', '.zcode')
$rc = Invoke-Native { robocopy $Repo $srcDst /E /NFL /NDL /NJH /NJS /NP /XD @excl }
# robocopy: 0-7 成功(1=有文件被复制), >=8 才是错误
if ($rc.Code -ge 8) { throw "robocopy 失败 (exit $($rc.Code))" }

# 3) 本地独有文件(不入库但重要 —— AGENTS.md 是项目唯一记忆源)
Write-Step "3/5 复制本地独有文件…"
foreach ($f in 'AGENTS.md', 'config.yaml', 'known_hosts.json') {
  $p = Join-Path $Repo $f
  if (Test-Path $p) { Copy-Item $p (Join-Path $dst $f) -Force; Write-Step "    + $f" }
}

# 4) 清单:便于日后回溯"这是哪一版、当时是否干净"
Write-Step "4/5 写清单…"
$status = Invoke-Git status --porcelain
$dirty = @($status.Out -split "`n" | Where-Object { $_.Trim() }).Count
$headLine = (Invoke-Git log -1 '--format=%h %s').Out.Trim()
$originLine = (Invoke-Git log -1 '--format=%h' origin/main).Out.Trim()
$ahead = (Invoke-Git rev-list --count origin/main..HEAD).Out.Trim()
$branch = (Invoke-Git rev-parse --abbrev-ref HEAD).Out.Trim()
$manifest = @(
  "快照时间   : $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')"
  "git HEAD   : $headLine"
  "origin/main: $originLine"
  "领先远端   : $ahead 个提交"
  "分支       : $branch"
  "工作区     : $(if ($dirty -eq 0) { '干净' } else { "$dirty 个文件有改动(未提交)" })"
  ""
  "还原方式   : git clone helm-repo.bundle helm-restored"
  "             git -C helm-restored remote set-url origin https://github.com/901548/Helm.git"
)
$manifest -join "`n" | Set-Content (Join-Path $dst '_MANIFEST.txt') -Encoding UTF8
$manifest | ForEach-Object { Write-Step "    $_" }

# 5) 清理:只保留最近 $Keep 份
Write-Step "5/5 清理旧快照(保留最近 $Keep 份)…"
$all = @(Get-ChildItem $BackupRoot -Directory -Filter 'snapshot-*' | Sort-Object Name -Descending)
if ($all.Count -gt $Keep) {
  foreach ($old in $all[$Keep..($all.Count - 1)]) {
    Remove-Item $old.FullName -Recurse -Force
    Write-Step "    - 删除 $($old.Name)"
  }
}

$sz = [math]::Round(((Get-ChildItem $dst -Recurse -File | Measure-Object -Property Length -Sum).Sum) / 1MB, 1)
$cnt = (Get-ChildItem $dst -Recurse -File | Measure-Object).Count
Write-Host "完成: $name  ($sz MB / $cnt 文件)"
