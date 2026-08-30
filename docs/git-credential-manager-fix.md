# Git Credential Manager "字符串绑定无效" 修复方案

## 问题描述

运行 `git-credential-manager.exe` 时弹出系统错误窗口：

```
git-credential-manager.exe - 系统错误
字符串绑定无效
```

错误代码：`0xC0020001` (.NET CLR 错误)

---

## 第一阶段：诊断问题根源

### 步骤 1：检查 Git 和 GCM 版本

```powershell
# 检查 Git 版本
git --version

# 检查当前配置的 credential helper
git config --global credential.helper

# 列出所有 credential 相关配置
git config --global --list | findstr credential
```

### 步骤 2：定位 git-credential-manager.exe

```powershell
# 查找 GCM 可执行文件位置
where git-credential-manager.exe

# 或手动检查常见路径
dir "C:\Program Files\Git\mingw64\bin\git-credential-manager.exe" 2>nul
dir "C:\Program Files\Git\mingw64\libexec\git-core\git-credential-manager.exe" 2>nul
dir "C:\Program Files (x86)\Git\mingw64\bin\git-credential-manager-core.exe" 2>nul
```

### 步骤 3：检查 .NET Framework 版本

```powershell
# 检查已安装的 .NET 版本
dir "HKLM:\SOFTWARE\Microsoft\NET Framework Setup\NDP\v4\Full\" /v Version
```

### 步骤 4：启用 GCM 调试日志

```powershell
# 启用详细日志
set GCM_TRACE=1
set GCM_TRACE_SECRETS=1
set GIT_TRACE=1

# 然后执行 git 操作查看日志
git push origin main
```

### 步骤 5：检查 Windows 凭据管理器

```
1. 打开 控制面板 → 凭据管理器
2. 选择 "Windows 凭据"
3. 搜索包含 "git" 的条目
4. 记录所有 Git 相关的凭据条目
```

---

## 第二阶段：修复方案

### 方案 A：更新 Git Credential Manager（推荐首选）

```powershell
# 下载最新版 GCM
# 访问 https://github.com/git-ecosystem/git-credential-manager/releases

# 卸载旧版本（如果有）
git-credential-manager.exe uninstall

# 安装新版本
# 运行安装程序，选择 "Git for Windows" 作为部署目标
```

### 方案 B：重新配置 Credential Helper

```powershell
# 方案 1：使用 manager (新版 GCM)
git config --global credential.helper manager

# 方案 2：使用 store（简单存储，无加密）
git config --global credential.helper store

# 方案 3：使用 wincred（Windows 原生）
git config --global credential.helper wincred

# 方案 4：使用 cache（内存缓存）
git config --global credential.helper cache
```

### 方案 C：清理损坏的凭据

```powershell
# 方法 1：使用 GCM 命令删除特定凭据
git-credential-manager erase --host=github.com
git-credential-manager erase --host=gitlab.com
git-credential-manager erase --host=dev.azure.com

# 方法 2：通过控制面板
# 控制面板 → 凭据管理器 → Windows 凭据 → 删除所有 git 相关条目

# 方法 3：使用 cmdkey
cmdkey /list | findstr git
cmdkey /delete:git:https://github.com
```

### 方案 D：修复 PATH 环境变量

```powershell
# 将 GCM 所在目录添加到 PATH
set PATH=C:\Program Files\Git\mingw64\bin;%PATH%

# 永久添加（以管理员运行）
[System.Environment]::SetEnvironmentVariable("PATH", "C:\Program Files\Git\mingw64\bin;$env:PATH", "Machine")
```

### 方案 E：重新安装 Git for Windows

```powershell
# 1. 卸载当前 Git
# 控制面板 → 程序和功能 → Git for Windows → 卸载

# 2. 删除残留配置
del "%USERPROFILE%\.gitconfig" /f
del "%USERPROFILE%\.git-credentials" /f

# 3. 下载最新 Git for Windows
# 访问 https://git-scm.com/download/win

# 4. 重新安装，确保选择：
#    - "Git Credential Manager Core" 作为默认 credential helper
#    - 添加到 PATH

# 5. 重新配置 Git
git config --global user.name "Your Name"
git config --global user.email "your@email.com"
git config --global credential.helper manager
```

### 方案 F：临时绕过（用于 HuggingFace 等）

```powershell
# 将令牌直接嵌入 URL（注意安全风险）
git push https://username:token@huggingface.co/spaces/username/repo main

# 或使用 .netrc 文件
echo machine huggingface.co login username password token >> %USERPROFILE%\.netrc
```

---

## 第三阶段：验证解决方案

### 验证步骤

```powershell
# 1. 验证 GCM 配置
git config --global credential.helper
# 应输出: manager / store / wincred 等

# 2. 测试 credential 操作
echo url=https://github.com | git credential fill
# 输入用户名密码后应正常保存

# 3. 测试 git push/pull
git push origin main
git pull origin main

# 4. 验证凭据已存储
git-credential-manager get <<< "protocol=https
host=github.com"
```

### 成功标志

- ✅ `git push/pull` 不再弹出错误窗口
- ✅ 不再要求反复输入用户名密码
- ✅ 凭据管理器中显示有效的 Git 凭据

---

## 第四阶段：预防措施

### 1. 定期更新 Git

```powershell
# 检查更新
git update-git-for-windows
```

### 2. 避免手动修改 GCM 文件

```powershell
# GCM 位于 Git 安装目录内，避免手动删除或移动
# 升级 Git 时 GCM 会自动更新
```

### 3. 使用 Personal Access Token (PAT)

```powershell
# 为 GitHub 创建 PAT
# Settings → Developer settings → Personal access tokens → Generate

# 使用 PAT 代替密码
# 输入用户名后，PAT 作为密码输入
```

### 4. 定期清理旧凭据

```powershell
# 定期检查并删除过期/不用的凭据
git-credential-manager erase --host=unused-host.com
```

### 5. 备份 .gitconfig

```powershell
# 备份配置
copy "%USERPROFILE%\.gitconfig" "%USERPROFILE%\.gitconfig.backup"
```

---

## 常见问题解答

### Q: 为什么会出现 "字符串绑定无效"？

A: 这是 .NET CLR 错误 (0xC0020001)，通常由于：
- GCM 版本与 .NET Framework 不兼容
- GCM 可执行文件损坏
- 混合模式初始化失败

### Q: 更新 GCM 后仍有问题？

A: 尝试：
1. 完全卸载后重新安装
2. 清理所有 Git 凭据
3. 检查 PATH 是否正确

### Q: WSL 中遇到此问题？

A: WSL 需要特殊配置：
```bash
git config --global credential.helper "/mnt/c/Program\ Files/Git/mingw64/bin/git-credential-manager-core.exe"
```

---

## 参考资源

- [Git Credential Manager 官方文档](https://github.com/git-ecosystem/git-credential-manager)
- [GCM Issue #1895 - 字符串绑定无效](https://github.com/git-ecosystem/git-credential-manager/issues/1895)
- [Stack Overflow 解决方案](https://stackoverflow.com/questions/75949578/how-to-fix-git-for-windows-keeps-asking-for-credentials)
