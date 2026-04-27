@echo off
chcp 65001 >nul

echo [1/4] 正在直接下载最新的核心代码...
:: 使用对国内友好的 dpaste 下载
curl -sL https://dpaste.com/C9YZ49AFJ.txt -o src\main.rs

:: 检查文件是否下载成功（文件大小是否大于 0）
for %%I in (src\main.rs) do if %%~zI==0 set DOWNLOAD_FAILED=1
if defined DOWNLOAD_FAILED (
  echo [错误] 代码下载失败，请检查网络！
  pause
  exit /b 1
)

echo [2/4] 正在配置 GitHub 自动打包流水线...
if not exist .github\workflows mkdir .github\workflows
curl -sL https://dpaste.com/9CFSSRSXQ.txt -o .github\workflows\windows-build.yml

echo [3/4] 正在提交所有更新...
git add .
git commit -m "feat: apply all final optimizations via direct file override"

echo [4/4] 正在推送到 GitHub...
git push origin master

if %errorlevel% neq 0 (
  echo [错误] 推送失败：请检查网络或 SSH 设置
  pause
  exit /b %errorlevel%
)

echo ===============================================
echo [成功] 代码已全部更新并推送到 GitHub！
echo 既然你在本地编译有困难，你现在可以直接去你的 GitHub 页面，
echo 点击顶部的 Actions 标签，等待一分钟左右，
echo GitHub 服务器会自动帮你打包出最终版的 transglass.exe 安装包供你下载！
echo ===============================================
pause
