#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2025-2026 Seyud
# DevMask: https://github.com/uamind/DevMask

"""
DevMask WebUI 构建脚本
这个脚本自动化了 Vue.js 项目的常见构建任务
"""

import os
import shutil
import sys
import subprocess
import argparse
import logging
from pathlib import Path
from typing import Optional


def configure_utf8_console() -> None:
    """优先将控制台输出切换为 UTF-8，避免 Windows 下 emoji 输出导致构建脚本崩溃。"""
    for stream_name in ("stdout", "stderr"):
        stream = getattr(sys, stream_name, None)
        if stream is not None and hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")


configure_utf8_console()

# 设置日志
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)


def write_console_line(line: str) -> None:
    """稳健地向控制台输出一行，避免 Windows 控制台编码导致构建脚本崩溃。"""
    text = f"{line}\n"
    try:
        sys.stdout.write(text)
    except UnicodeEncodeError:
        encoding = sys.stdout.encoding or "utf-8"
        safe_text = text.encode(encoding, errors="replace").decode(encoding, errors="replace")
        sys.stdout.write(safe_text)
    sys.stdout.flush()

class WebUIBuilder:
    """WebUI 项目构建器"""
    
    def __init__(self, project_dir: Optional[str] = None):
        """初始化构建器
        
        Args:
            project_dir: 项目目录路径，默认为当前目录
        """
        self.project_dir = Path(project_dir) if project_dir else Path(__file__).parent
        # 根据 vite.config.ts 检测实际构建输出目录
        vite_config = self.project_dir / "vite.config.ts"
        if vite_config.exists():
            # 默认输出到 ../module/webroot
            self.dist_dir = self.project_dir / ".." / "module" / "webroot"
        else:
            self.dist_dir = self.project_dir / "dist"
        self.node_modules = self.project_dir / "node_modules"
        self.package_json = self.project_dir / "package.json"
        self.npm_path = None  # npm 可执行文件路径（由 check_node_npm 探测）
        
        if not self.package_json.exists():
            raise FileNotFoundError(f"在 {self.project_dir} 中找不到 package.json")
    
    @staticmethod
    def _spawn_args(command: list) -> list:
        """把命令列表改成可直接执行的 argv。

        Windows 上 .cmd / .bat 不能直接交给 CreateProcess（npm 就是 npm.cmd），
        统一用 `cmd.exe /c` 包一层；其它平台原样返回。
        """
        if os.name == "nt" and command and str(command[0]).lower().endswith((".cmd", ".bat")):
            comspec = os.environ.get("COMSPEC") or "cmd.exe"
            return [comspec, "/c", *command]
        return command

    def _probe_version(self, executable: str) -> Optional[str]:
        """跑 `<exe> --version`；能跑通就返回首行版本号，否则返回 None。"""
        try:
            completed = subprocess.run(
                self._spawn_args([executable, "--version"]),
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=120,
            )
        except (OSError, subprocess.SubprocessError):
            return None

        output = (completed.stdout or "").strip()
        if completed.returncode != 0 or not output:
            return None
        return output.splitlines()[0].strip()

    def check_node_npm(self) -> bool:
        """探测 node / npm。

        顺序：先看 PATH（macOS / Linux / 已把 Node.js 加进 PATH 的 Windows 都走这条），
        Windows 再回退到常见安装目录（包含历史上硬编码的 D:\nodejs）。
        探测到的 npm 绝对路径存进 self.npm_path，后续所有 npm 调用都用它。
        """
        npm_candidates: list = []
        node_candidates: list = []

        for name in ("npm", "npm.cmd", "npm.exe"):
            found = shutil.which(name)
            if found and found not in npm_candidates:
                npm_candidates.append(found)
        for name in ("node", "node.exe"):
            found = shutil.which(name)
            if found and found not in node_candidates:
                node_candidates.append(found)

        if os.name == "nt":
            # Windows 的 Node.js 安装器经常只写用户 PATH，这里保留目录兜底。
            for base in (
                os.environ.get("NODE_HOME"),
                r"D:\nodejs",
                r"C:\Program Files\nodejs",
                r"C:\Program Files (x86)\nodejs",
            ):
                if not base:
                    continue
                for exe in ("npm.cmd", "npm.exe"):
                    candidate = os.path.join(base, exe)
                    if os.path.exists(candidate) and candidate not in npm_candidates:
                        npm_candidates.append(candidate)
                for exe in ("node.exe", "node"):
                    candidate = os.path.join(base, exe)
                    if os.path.exists(candidate) and candidate not in node_candidates:
                        node_candidates.append(candidate)

        if not npm_candidates:
            logger.error(
                "未找到 npm：请安装 Node.js（https://nodejs.org/）并确保 node / npm 在 PATH 中；"
                "Windows 也可以安装在 D:\\nodejs"
            )
            return False

        npm_version = None
        for candidate in npm_candidates:
            npm_version = self._probe_version(candidate)
            if npm_version:
                self.npm_path = candidate
                break

        if not npm_version:
            logger.error(
                "npm 存在但无法执行：" + "、".join(npm_candidates) + "；请检查 Node.js 安装是否完整"
            )
            return False

        logger.info(f"npm 路径: {self.npm_path}")
        logger.info(f"npm 版本: {npm_version}")

        for candidate in node_candidates:
            node_version = self._probe_version(candidate)
            if node_version:
                logger.info(f"Node.js 版本: {node_version}（{candidate}）")
                break

        return True
    
    def install_dependencies(self) -> bool:
        """安装 npm 依赖"""
        if self.node_modules.exists():
            logger.info("依赖已存在，跳过安装")
            return True
        
        logger.info("正在安装 npm 依赖...")
        return self.run_command(["npm", "install"], "npm 依赖安装")
    
    def run_command(self, command: list, description: str, use_npm: bool = True) -> bool:
        """执行命令并返回是否成功，实时显示输出
        
        Args:
            command: 要执行的命令列表
            description: 命令描述
            use_npm: 是否使用npm路径
        """
        logger.info(f"正在执行: {description}")
        
        # npm 一律用探测到的绝对路径（PATH 优先，Windows 回退到常见安装目录）
        if use_npm and command and command[0] == "npm":
            command[0] = self.npm_path or "npm"

        try:
            # 使用 Popen 实现实时输出
            process = subprocess.Popen(
                self._spawn_args(command),
                cwd=self.project_dir,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,  # 合并 stderr 到 stdout
                text=True,
                encoding='utf-8',
                errors='ignore',
                bufsize=1,  # 行缓冲
                universal_newlines=True
            )
            
            # 确保 stdout 不为 None
            if process.stdout is None:
                logger.error(f"{description} 失败: 无法获取输出流")
                return False
            
            # 实时读取并输出
            while True:
                line = process.stdout.readline()
                if line:
                    write_console_line(line.rstrip())
                elif process.poll() is not None:
                    # 进程已结束，读取剩余输出
                    remaining = process.stdout.read()
                    if remaining:
                        for extra_line in remaining.splitlines():
                            write_console_line(extra_line)
                    break
            
            return_code = process.returncode
            
            if return_code == 0:
                logger.info(f"{description} 完成")
            else:
                logger.error(f"{description} 失败，返回码: {return_code}")
            
            return return_code == 0
            
        except FileNotFoundError:
            logger.error(f"{description} 失败: 找不到命令 {command[0]}")
            return False
        except Exception as e:
            logger.error(f"{description} 失败: {e}")
            return False
    
    def dev(self) -> None:
        """启动开发服务器"""
        if not self._ensure_dependencies():
            return
        logger.info("启动开发服务器...")
        
        command = [self.npm_path or "npm", "run", "dev"]

        try:
            subprocess.run(self._spawn_args(command), cwd=self.project_dir)
        except KeyboardInterrupt:
            logger.info("开发服务器已停止")
    
    def build(self) -> bool:
        """构建生产版本"""
        if not self.check_node_npm():
            return False

        logger.info("开始构建生产版本...")

        # 步骤 0: 依赖一致性检查与安装（复现 CI 的 npm ci，提前发现 lock 不同步）
        # CI 第 8 步用 `npm ci` 严格校验 package.json 与 package-lock.json 是否同步，
        # 本地预先跑一遍可在提交前暴露 "Missing ... from lock file" 之类的问题。
        logger.info("=== 依赖一致性检查 (npm ci) ===")
        ci_check = self.run_command(
            ["npm", "ci", "--no-audit", "--no-fund"],
            "npm ci 依赖一致性校验",
            use_npm=True,
        )
        if not ci_check:
            logger.error("❌ 依赖一致性检查失败：package.json 与 package-lock.json 不同步")
            logger.error("   请在 webui 目录运行 `npm install` 更新 lock 文件后再提交，避免 CI 失败")
            return False
        logger.info("✅ 依赖一致性检查通过")

        # 步骤 1: TypeScript 类型检查（强制检查，失败时停止构建）
        logger.info("=== TypeScript 类型检查 ===")
        type_check_success = self.run_command(["npm", "run", "type-check"], "TypeScript 类型检查", use_npm=True)
        if not type_check_success:
            logger.error("❌ TypeScript 类型检查失败，构建终止")
            return False
        logger.info("✅ TypeScript 类型检查通过")
        
        # 步骤 2: 代码检查（失败时自动修复后重试）
        logger.info("=== 代码检查 ===")
        lint_success = self.run_command(["npm", "run", "lint"], "代码检查", use_npm=True)
        if not lint_success:
            logger.warning("❌ 代码检查失败，尝试自动修复...")
            # 尝试自动修复
            fix_success = self.run_command(["npm", "run", "lint:fix"], "代码检查自动修复", use_npm=True)
            if not fix_success:
                logger.error("❌ 自动修复失败，构建终止")
                return False
            
            # 重新检查
            logger.info("=== 重新检查代码 ===")
            lint_success = self.run_command(["npm", "run", "lint"], "代码检查（修复后）", use_npm=True)
            if not lint_success:
                logger.error("❌ 修复后代码检查仍失败，构建终止")
                return False
        
        logger.info("✅ 代码检查通过")
        
        # 步骤 3: 构建生产版本
        logger.info("=== 构建生产版本 ===")
        success = self.run_command(["npm", "run", "build"], "构建生产版本", use_npm=True)
        
        if success and self.dist_dir.exists():
            dist_size = self._get_directory_size(self.dist_dir)
            logger.info(f"构建完成！输出目录: {self.dist_dir}")
            logger.info(f"构建产物大小: {dist_size}")
        else:
            logger.error("构建失败")
        
        return success
    

    def lint(self, fix: bool = False) -> bool:
        """运行代码检查
        
        Args:
            fix: 是否自动修复问题
        """
        command = ["npm", "run", "lint"]
        if fix:
            command = ["npm", "run", "lint:fix"]
        
        return self.run_command(command, "代码检查" + (" (自动修复)" if fix else ""), use_npm=True)
    
    def format(self, check: bool = False) -> bool:
        """格式化代码
        
        Args:
            check: 是否只检查格式，不实际修改
        """
        command = ["npm", "run", "format"]
        if check:
            command = ["npm", "run", "format:check"]
        
        return self.run_command(command, "代码格式化" + (" (检查)" if check else ""), use_npm=True)
    
    def type_check(self) -> bool:
        """运行 TypeScript 类型检查"""
        return self.run_command(["npm", "run", "type-check"], "TypeScript 类型检查", use_npm=True)
    
    def clean(self) -> None:
        """清理构建产物"""
        if self.dist_dir.exists():
            import shutil
            shutil.rmtree(self.dist_dir)
            logger.info("已清理构建产物")
        
        if self.node_modules.exists():
            import shutil
            # 只在明确要求清理依赖时才清理
            logger.info("注意：node_modules 未删除，如需清理请使用 --clean-deps 参数")
    
    def clean_all(self) -> None:
        """清理所有生成的文件"""
        import shutil
        
        # 清理构建产物
        if self.dist_dir.exists():
            shutil.rmtree(self.dist_dir)
            logger.info("已清理构建产物")
        
        # 清理 node_modules
        if self.node_modules.exists():
            shutil.rmtree(self.node_modules)
            logger.info("已清理依赖")
        
        # 清理其他可能的缓存文件
        cache_dirs = [".vite", ".cache", "coverage"]
        for cache_dir in cache_dirs:
            cache_path = self.project_dir / cache_dir
            if cache_path.exists():
                shutil.rmtree(cache_path)
                logger.info(f"已清理 {cache_dir}")
    
    def _ensure_dependencies(self) -> bool:
        """确保依赖已安装"""
        if not self.check_node_npm():
            return False
        
        if not self.node_modules.exists():
            return self.install_dependencies()
        
        return True
    
    def _get_directory_size(self, directory: Path) -> str:
        """获取目录大小"""
        total_size = 0
        for file_path in directory.rglob('*'):
            if file_path.is_file():
                total_size += file_path.stat().st_size
        
        # 转换为更友好的单位
        for unit in ['B', 'KB', 'MB', 'GB']:
            if total_size < 1024:
                return f"{total_size:.1f} {unit}"
            total_size /= 1024
        return f"{total_size:.1f} TB"
def main():
    """主函数"""
    parser = argparse.ArgumentParser(description="DevMask WebUI 构建脚本")
    parser.add_argument(
        "command",
        nargs='?',  # 可选参数
        default='build',  # 默认执行构建
        choices=["dev", "build", "lint", "lint:fix", "format", "format:check", "type-check", "clean", "clean:all"],
        help="要执行的命令（不指定时默认执行构建）"
    )
    parser.add_argument(
        "--project-dir",
        type=str,
        help="项目目录路径（默认为当前目录）"
    )
    parser.add_argument(
        "--skip-deps-check",
        action="store_true",
        help="跳过依赖检查"
    )
    
    args = parser.parse_args()
    
    try:
        builder = WebUIBuilder(args.project_dir)
        
        # 默认执行完整构建流程（类似 build_android.py）
        if args.command == "build":
            write_console_line("机型伪装 WebUI 构建脚本")
            write_console_line("=" * 50)
            
            write_console_line("")
            write_console_line("=== 执行构建流程 ===")
            if not builder.build():
                write_console_line("❌ 构建失败")
                sys.exit(1)
            
            write_console_line("")
            write_console_line("=" * 50)
            write_console_line("✅ 构建完成！")
            write_console_line(f"构建产物位于 {builder.dist_dir} 目录")
            write_console_line("")
            write_console_line("构建产物已集成到模块中")
            write_console_line("=" * 50)
        
        elif args.command == "dev":
            builder.dev()

        elif args.command == "lint":
            success = builder.lint()
            sys.exit(0 if success else 1)
        elif args.command == "lint:fix":
            success = builder.lint(fix=True)
            sys.exit(0 if success else 1)
        elif args.command == "format":
            success = builder.format()
            sys.exit(0 if success else 1)
        elif args.command == "format:check":
            success = builder.format(check=True)
            sys.exit(0 if success else 1)
        elif args.command == "type-check":
            success = builder.type_check()
            sys.exit(0 if success else 1)
        elif args.command == "clean":
            builder.clean()
        elif args.command == "clean:all":
            builder.clean_all()

    
    except KeyboardInterrupt:
        logger.info("操作被用户中断")
        sys.exit(1)
    except Exception as e:
        logger.error(f"执行过程中发生错误: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
