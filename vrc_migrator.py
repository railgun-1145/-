import os
import sys
import shutil
import subprocess
import ctypes
import tkinter as tk
from tkinter import messagebox, filedialog
import threading

# 路径定义
USER_PROFILE = os.environ.get('USERPROFILE')
DEFAULT_C_PATH = os.path.join(USER_PROFILE, 'AppData', 'LocalLow', 'VRChat', 'VRChat')
DEFAULT_D_PATH = r"D:\VRChat\Cache"

def is_admin():
    try:
        return ctypes.windll.shell32.IsUserAnAdmin()
    except:
        return False

class VRCMigratorGUI:
    def __init__(self, root):
        self.root = root
        self.root.title("VRChat 缓存一键迁移工具 (可视化版)")
        self.root.geometry("600x450")
        self.root.resizable(False, False)

        # 状态变量
        self.status_text = tk.StringVar(value="等待操作...")
        
        self.setup_ui()
        
    def setup_ui(self):
        # 标题栏
        title_label = tk.Label(self.root, text="VRChat 缓存/模型 自动迁移工具", font=("Microsoft YaHei", 16, "bold"), pady=20)
        title_label.pack()

        # 路径显示框
        path_frame = tk.LabelFrame(self.root, text="路径设置", padx=10, pady=10)
        path_frame.pack(fill="x", padx=20)

        tk.Label(path_frame, text="C 盘原始路径:").grid(row=0, column=0, sticky="w")
        self.c_entry = tk.Entry(path_frame, width=50)
        self.c_entry.insert(0, DEFAULT_C_PATH)
        self.c_entry.grid(row=0, column=1, padx=5)
        self.c_entry.config(state='readonly')

        tk.Label(path_frame, text="D 盘目标路径:").grid(row=1, column=0, sticky="w")
        self.d_entry = tk.Entry(path_frame, width=50)
        self.d_entry.insert(0, DEFAULT_D_PATH)
        self.d_entry.grid(row=1, column=1, padx=5)
        
        btn_browse = tk.Button(path_frame, text="浏览", command=self.browse_d_path)
        btn_browse.grid(row=1, column=2)

        # 状态显示区域
        status_frame = tk.LabelFrame(self.root, text="运行日志", padx=10, pady=10)
        status_frame.pack(fill="both", expand=True, padx=20, pady=10)

        self.log_area = tk.Text(status_frame, height=8, state='disabled', font=("Consolas", 9))
        self.log_area.pack(fill="both", expand=True)

        # 按钮区域
        btn_frame = tk.Frame(self.root, pady=10)
        btn_frame.pack()

        self.btn_migrate = tk.Button(btn_frame, text="🚀 一键迁移到 D 盘", command=self.start_migration, 
                                     bg="#4CAF50", fg="white", font=("Microsoft YaHei", 10, "bold"), width=20)
        self.btn_migrate.pack(side="left", padx=10)

        self.btn_restore = tk.Button(btn_frame, text="🔙 还原到 C 盘", command=self.start_restore,
                                     bg="#f44336", fg="white", font=("Microsoft YaHei", 10, "bold"), width=20)
        self.btn_restore.pack(side="left", padx=10)

        # 版权/安全说明
        tk.Label(self.root, text="* 本工具基于系统符号链接功能，不修改游戏数据，100%安全不封号 *", 
                 fg="gray", font=("Microsoft YaHei", 8)).pack(pady=5)

    def log(self, message):
        self.log_area.config(state='normal')
        self.log_area.insert(tk.END, f"> {message}\n")
        self.log_area.see(tk.END)
        self.log_area.config(state='disabled')
        self.root.update()

    def browse_d_path(self):
        directory = filedialog.askdirectory()
        if directory:
            self.d_entry.delete(0, tk.END)
            self.d_entry.insert(0, directory)

    def close_vrchat(self):
        self.log("正在尝试关闭 VRChat...")
        try:
            subprocess.run(["taskkill", "/F", "/IM", "VRChat.exe"], capture_output=True)
            self.log("已发送关闭命令（如果运行中）。")
        except Exception as e:
            self.log(f"关闭进程时出错: {e}")

    def start_migration(self):
        if not is_admin():
            messagebox.showerror("权限不足", "请以管理员权限运行此工具！")
            return
        
        target_d = self.d_entry.get().strip()
        if not target_d:
            messagebox.showwarning("提示", "目标路径不能为空")
            return

        if messagebox.askyesno("确认", f"确定要将缓存迁移到 {target_d} 吗？\n迁移过程中请确保 VRChat 已关闭。"):
            threading.Thread(target=self.run_migration_logic, args=(target_d,), daemon=True).start()

    def move_files(self, src, dst):
        """通用的文件迁移函数，处理目录结构"""
        if not os.path.exists(src):
            return
        
        # 遍历源目录
        for root_dir, dirs, files in os.walk(src):
            # 获取相对路径
            rel_path = os.path.relpath(root_dir, src)
            target_dir = os.path.join(dst, rel_path)
            
            if not os.path.exists(target_dir):
                os.makedirs(target_dir)
            
            for f in files:
                s_file = os.path.join(root_dir, f)
                d_file = os.path.join(target_dir, f)
                
                # 如果目标文件已存在，先删除（或者覆盖）
                if os.path.exists(d_file):
                    os.remove(d_file)
                
                # 移动文件
                shutil.move(s_file, d_file)
        
        # 删除空的原目录树
        shutil.rmtree(src)

    def run_migration_logic(self, target_d):
        self.btn_migrate.config(state='disabled')
        self.btn_restore.config(state='disabled')
        
        try:
            self.close_vrchat()
            
            # 1. 检查目标目录
            if not os.path.exists(target_d):
                self.log(f"创建目标目录: {target_d}")
                os.makedirs(target_d, exist_ok=True)

            # 2. 处理原 C 盘目录
            if os.path.exists(DEFAULT_C_PATH):
                # 如果已经是软链接，先删除链接
                if self.is_junction(DEFAULT_C_PATH):
                    self.log("发现已有链接，正在清理旧链接以更新目标...")
                    os.rmdir(DEFAULT_C_PATH)
                else:
                    self.log("正在迁移文件（可能需要几分钟，请稍候）...")
                    self.move_files(DEFAULT_C_PATH, target_d)
                    self.log("文件迁移完成。")

            # 3. 创建 Junction 软链接
            self.log("创建系统链接 (Junction)...")
            # 确保父目录存在
            os.makedirs(os.path.dirname(DEFAULT_C_PATH), exist_ok=True)
            cmd = f'mklink /J "{DEFAULT_C_PATH}" "{target_d}"'
            res = subprocess.run(cmd, shell=True, capture_output=True, text=True)
            
            if res.returncode == 0:
                self.log("✅ 迁移成功！")
                messagebox.showinfo("成功", f"迁移完成！\n现在所有缓存都存储在: {target_d}")
            else:
                self.log(f"❌ 创建链接失败: {res.stderr}")
                messagebox.showerror("失败", f"创建链接失败: {res.stderr}")

        except Exception as e:
            self.log(f"程序运行发生错误: {str(e)}")
            messagebox.showerror("错误", str(e))
        finally:
            self.btn_migrate.config(state='normal')
            self.btn_restore.config(state='normal')

    def start_restore(self):
        if not is_admin():
            messagebox.showerror("权限不足", "请以管理员权限运行此工具！")
            return

        if messagebox.askyesno("确认", "确定要还原缓存到 C 盘吗？"):
            threading.Thread(target=self.run_restore_logic, daemon=True).start()

    def run_restore_logic(self):
        self.btn_migrate.config(state='disabled')
        self.btn_restore.config(state='disabled')
        target_d = self.d_entry.get().strip()

        try:
            self.close_vrchat()

            if os.path.exists(DEFAULT_C_PATH):
                if self.is_junction(DEFAULT_C_PATH):
                    self.log("正在删除软链接...")
                    os.rmdir(DEFAULT_C_PATH)
                    
                    self.log("正在从 D 盘拷回文件到 C 盘...")
                    os.makedirs(DEFAULT_C_PATH, exist_ok=True)
                    if os.path.exists(target_d):
                        self.move_files(target_d, DEFAULT_C_PATH)
                        self.log("✅ 还原成功！")
                        messagebox.showinfo("成功", "已成功还原到 C 盘！")
                    else:
                        self.log("⚠️ 警告：未发现 D 盘备份文件，仅移除了链接。")
                else:
                    self.log("C 盘目录看起来不是链接，无需还原。")
            else:
                self.log("C 盘链接已不存在。")
                if os.path.exists(target_d):
                    self.log("正在将 D 盘文件恢复至 C 盘...")
                    self.move_files(target_d, DEFAULT_C_PATH)
                    self.log("✅ 恢复成功！")
                    messagebox.showinfo("成功", "已将文件从 D 盘恢复至 C 盘！")
                
        except Exception as e:
            self.log(f"还原时出错: {str(e)}")
            messagebox.showerror("错误", str(e))
        finally:
            self.btn_migrate.config(state='normal')
            self.btn_restore.config(state='normal')

    def is_junction(self, path):
        """检查路径是否为 Junction (符号链接/目录联接)"""
        if not os.path.exists(path):
            return False
        # Windows 特有的检查方式
        try:
            # 目录联接或符号链接通常有特殊属性
            # os.path.islink 在 Windows 3.8+ 对 Junction 返回 True
            if os.path.islink(path):
                return True
            # 如果不确定，可以尝试查询 reparse point
            output = subprocess.check_output(['fsutil', 'reparsepoint', 'query', path], stderr=subprocess.STDOUT, shell=True)
            return b"Reparse Tag Value : 0xa0000003" in output or b"Mount Point" in output
        except:
            return False

if __name__ == "__main__":
    if not is_admin():
        # 重新以管理员身份运行
        ctypes.windll.shell32.ShellExecuteW(None, "runas", sys.executable, " ".join(sys.argv), None, 1)
    else:
        root = tk.Tk()
        app = VRCMigratorGUI(root)
        root.mainloop()
