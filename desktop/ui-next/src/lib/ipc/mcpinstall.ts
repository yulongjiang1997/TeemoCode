// MCP 一键装:把仓库扫描到的 MCP 服务器配置批量安装到 config。
// 命令对表 desktop/src/main.rs mcp_servers_install;浏览器模式抛错。
import { inDesktopShell, invoke } from "./ipc";

export async function mcpServersInstall(entries: Record<string, Record<string, unknown>>): Promise<string> {
  if (!inDesktopShell()) throw new Error("浏览器模式下不支持 MCP 安装");
  return invoke<string>("mcp_servers_install", { entries });
}
