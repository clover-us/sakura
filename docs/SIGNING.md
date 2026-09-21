# 代码签名（Windows）怎么搞

安装包现在**没有签名**，所以用户第一次跑会看到 SmartScreen 的「未知发布者」。
这份文档记的是：有哪些路子、各花多少钱、以及**这个仓库具体怎么接**。
（信息核对于 2026-09-21，来源见文末；这类政策变动频繁，动手前请再确认一次。）

## 一、先明确它解决什么、不解决什么

| 做了签名 | 没做签名 |
| --- | --- |
| 安装时显示**发布者名字**（你的证书主体名），而不是"未知发布者" | 一律显示「未知发布者」，用户得点"仍要运行" |
| 让签名身份**积累 SmartScreen 信誉**（同一个身份连续发版，后面的版本能继承信任） | 每次都是全新文件，永远从零开始 |
| 文件被篡改能被发现（签名验证） | 无完整性保证 |
| 企业环境里更容易放行（很多企业策略直接拦未签名程序） | 容易被企业策略拦掉 |

**签了不等于立刻没警告**：2024 年微软改了规则——EV 证书**不再**一签就免 SmartScreen
（以前 EV 有这个特权），现在 OV/EV 都是"先警告，靠同一个发布者身份连续发版慢慢积累信誉"。
所以**不要为了 SmartScreen 去多花钱买 EV**。

## 二、四条路（按"对个人/小项目现实不现实"排）

| 路子 | 花费 | 谁能办 | 对本项目的现实性 |
| --- | --- | --- | --- |
| **微软商店（MSIX）** | 免费（商店**替你重签**） | 全球，注册个开发者账号 | 要先把应用打成 MSIX——Tauri 目前只出 exe/NSIS/MSI，得自己用 `makeappx` 包一层；走 MSI/EXE 提交通道则**仍要自己签**。工作量最大 |
| **Azure Artifact Signing**（原名 Trusted Signing） | 约 **$9.99/月** | 组织：美/加/欧盟/英；**个人：只有美国和加拿大** | ❌ **办不了**（本机在中国），除非有以上地区的实体 |
| **OV 证书**（传统 CA） | **$150–300/年**（国内转售约 ¥1000–3000） | 全球，个人可申请 | ✅ **现实路径**。2023-06 起私钥必须放 **USB token 或云 HSM**，所以出包时得插着 token（或走 CA 的云签工具） |
| **EV 证书** | $400+/年 | 全球 | ❌ 不划算：2024 起不再免 SmartScreen，纯粹多花钱 |
| **SignPath Foundation** | 免费（OV 级别 signing） | **公开开源项目**可申请 | 🤔 仓库若公开且符合条件，值得试；要接它的 CI 流水线 |
| **自签名证书** | 免费 | 自己签 | 只适合自测/内网（用户机器不信任它，照样拦） |

> 国内买 OV 代码签名证书的常见渠道：DigiCert / Sectigo / GlobalSign 的国内转售
> （阿里云、腾讯云、各大 CA 代理都有"代码签名证书"这一项，注意必须是 **code signing**，
> SSL 证书不行）。买之前问清三件事：**给不给 USB token**、**能不能签 exe + msi 两种**、
> **时间戳服务地址**。

## 三、这个仓库怎么接（拿到证书之后）

### 3.1 证书进"当前用户"证书库，拿到指纹

```powershell
# 有 .pfx 的话（很多 CA 直接给 token，那就插上 token、跳到第 2 步）
Import-PfxCertificate -FilePath cert.pfx -CertStoreLocation Cert:\CurrentUser\My `
  -Password (ConvertTo-SecureString '导出密码' -AsPlainText -Force)

# 看代码签名证书的指纹（SHA1）
Get-ChildItem Cert:\CurrentUser\My |
  Where-Object { $_.EnhancedKeyUsageList.ObjectId -contains '1.3.6.1.5.5.7.3.3' } |
  Format-List Subject, NotAfter, Thumbprint
```

### 3.2 `src-tauri/tauri.conf.json` 的 `bundle.windows` 加三个字段

签名参数就在这一处（Tauri 会把 **app exe、NSIS 安装器、MSI** 都签一遍）：

```json
"windows": {
  "wix": { "language": "zh-CN" },
  "nsis": { "languages": ["SimpChinese", "English"], "installerIcon": "icons/icon.ico", "uninstallerIcon": "icons/icon.ico" },
  "certificateThumbprint": "<上面查到的 SHA1 指纹，去掉空格>",
  "digestAlgorithm": "sha256",
  "timestampUrl": "http://timestamp.digicert.com",
  "tsp": true
}
```

- `tsp: true` 才会用 RFC3161 时间戳（`/tr` + `/td`）；不加就是老的 `/t`。
  **时间戳必须能连上**（本机网络走代理，出包机器要能访问那个地址），否则签名会失败；
- `certificateThumbprint` 是**机器相关**的，不建议长期提交进仓库——它只在本机有效，
  换机器/证书过期后构建会直接报错。可以出包时临时填，或者用 3.3 的自定义命令走环境变量。

### 3.3 或者用自定义签名命令（推荐给"证书在云上/token 里"的情况）

Tauri 支持 `signCommand`（`%1` 会被替换成待签文件路径）：

```json
"signCommand": {
  "cmd": "powershell",
  "args": ["-ExecutionPolicy", "Bypass", "-File", "scripts/sign-file.ps1", "%1"]
}
```

这样证书指纹、密码、时间戳地址都放在 `scripts/sign-file.ps1` 里（从环境变量读），
仓库里不落任何敏感值。Azure Artifact Signing 走的就是这条路（`signtool` + `/dlib` +
metadata json，见 Tauri 文档的 "Azure Artifact Signing" 一节）。

### 3.4 `signtool.exe` 从哪来（**本机现在没有**）

Tauri 自己找 `signtool`：先看环境变量 `TAURI_WINDOWS_SIGTOOL_PATH`，再查注册表
`HKLM\SOFTWARE\Microsoft\Windows Kits\Installed Roots` 的 `KitsRoot10`（Windows SDK 的安装路径）。
本机既没装 Windows SDK、也没有任何代码签名证书，所以**要签名得先装**
"Windows SDK – Signing Tools for Desktop Apps"（只勾这一个组件就够，别装整个 SDK），
或者把 `TAURI_WINDOWS_SIGTOOL_PATH` 指到一个现成的 `signtool.exe`。

### 3.5 验证签名

```powershell
& "$env:ProgramFiles(x86)\Windows Kits\10\bin\10.0.22621.0\x64\signtool.exe" verify /pa /v `
  src-tauri\target\x86_64-pc-windows-gnu\release\bundle\nsis\whale-pet_1.0.0_x64-setup.exe

# 或看资源管理器：exe 属性 → 数字签名
```

## 四、没证书也能先把管线跑通（自签名演练）

```powershell
$c = New-SelfSignedCertificate -Type CodeSigningCert -Subject 'CN=whale-pet dev' `
  -CertStoreLocation Cert:\CurrentUser\My -NotAfter (Get-Date).AddYears(1)
$c.Thumbprint                     # 填进 tauri.conf.json 的 certificateThumbprint
# 然后正常出包：tauri.cmd build --target x86_64-pc-windows-gnu
```

出来的安装**在本机仍然显示"未知发布者"**（除非把这张证书装进"受信任的根证书颁发机构"），
但能证明"配置 → signtool → 产物带签名"这条链路是通的（`signtool verify` 会告诉你
"证书链不受信任"，而不是"没有签名"）。

## 五、几个容易踩的点

| 坑 | 说明 |
| --- | --- |
| 以为 EV 能免警告 | 2024 起不能了，OV/EV 一样要积累信誉（[微软说明](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options)） |
| 时间戳连不上 | 出包机器必须能访问 TSA 地址；失败时签名整体失败（签名里没有时间戳，证书一过期签名就失效） |
| 只签 exe 忘了签安装器 | 用户下载的是 `setup.exe`，**它才是被 SmartScreen 检查的那个**；Tauri 三个都会签，手写脚本时别漏 |
| 卸载器签不了 | `uninstall.exe` 是 NSIS 在**用户机器上**生成的，出包时不存在，签不了（正常现象） |
| 每次换证书 | 换证书 = 换身份 = SmartScreen 信誉从零开始，所以**尽量别频繁换 CA/证书** |
| 在企业里被拦 | 企业常用"允许的发布者"白名单，没签名会被直接拦；签了也要 IT 把发布者加进策略 |

## 六、本项目的现状与下一步

- 现状：**没有证书、没有 signtool**，`bundle.windows` 里一个签名字段都没配（24 节之前的包都是裸的）；
- 短期（不花钱）：接受"未知发布者"，在发布说明里写清楚；想让某些机器不提示，
  只能卸载时手动把 exe 加进本机的"受信任的发布者"（内网做法，不适合分发给用户）；
- 花钱的正路：买 **OV 代码签名证书**（约 ¥1000–3000/年，含 token）→ 按第三节接上 → 每次发版都用**同一张证书**签；
- 顺带能拿到的收益：签了之后 Tauri 的自动更新（`tauri-plugin-updater`）才有意义——
  没签名的更新包在 Windows 上一样会被拦。

---
来源：
[微软：Code signing options for Windows app developers](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options)（2026-08-29 更新）、
[微软：SmartScreen reputation](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation)、
[Tauri：Windows Code Signing](https://v2.tauri.app/distribute/sign/windows/)、
[tauri-bundler sign.rs（字段与 signtool 查找逻辑）](https://docs.rs/tauri-bundler/latest/src/tauri_bundler/bundle/windows/sign.rs.html)、
[SignPath Foundation](https://signpath.io)。
