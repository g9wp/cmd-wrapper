# Cmd-Wrapper

`Cmd-Wrapper` 是一个高度可定制的 Rust CLI 工具，旨在通过简单的 **TOML** 配置文件对程序调用进行“重定向”与“参数重映射”。它可以将复杂的脚本调用隐藏在简单的可执行文件之后，并支持环境变量替换、参数解析及执行日志记录。

## 🚀 核心功能

*   **无感知封装**：根据可执行文件名自动匹配 `.toml` 配置文件。
*   **灵活的参数映射**：支持位置参数 (`$arg1`)、命名参数 (`-k v`) 、开关参数 (`-f`)和参数数组展开（多参数 `${...args}` / 单参数 `$(...args)`）。
*   **可自定义替换符号**：占位符前缀默认 `$`，可通过 `var_symbol` 改为 `%` 等，避免与 bash/fish 等脚本自身的 `$var` 语法冲突。
*   **环境变量支持**：支持从系统环境、TOML 内部定义进行动态替换。
*   **IO 流控制**：自由配置 `stdin`、`stdout`、`stderr` 的继承或丢弃。
*   **Shell 转义保护**：可选的 `shell_quote` 模式，防止带空格的参数解析失败。
*   **执行审计**：支持记录所有生成的最终命令及执行时间。

---

## 🛠️ 安装与编译

确保你已安装 Rust 工具链。

1.  克隆或创建项目。
2.  在项目根目录运行：
    ```bash
    cargo build --release
    ```
3.  从 `target/release/` 复制二进制文件到你的 PATH 目录。

---

## 📖 配置指南

工具会寻找与自身 **同名** 但后缀为 `.toml` 的文件。例如，你将程序重命名为 `run-fish.exe`，它就会读取 `run-fish.toml`。

### TOML 字段说明

| 字段 | 类型 | 说明 | 默认值 |
| :--- | :--- | :--- | :--- |
| `prog` | String | **必填**。实际要执行的程序路径或名称。 | - |
| `args` | Array | 要传递给程序的参数模板列表。支持变量替换。 | `[]` |
| `var_symbol` | String | 变量替换前缀符号。改为 `%` 等可避免与目标脚本的 `$var` 语法冲突（此时 `$` 原样传递）。 | `"$"` |
| `env` | Table | 键值对映射，定义额外的环境变量。 | `{}` |
| `shell_quote` | Bool | 是否自动对替换后的变量进行引号转义。 | `false` |
| `stdin` | String | 标准输入模式: `inherit`, `null`, `piped` | `inherit` |
| `stdout` | String | 标准输出模式: `inherit`, `null`, `piped` | `inherit` |
| `stderr` | String | 标准错误模式: `inherit`, `null`, `piped` | `inherit` |
| `log_file` | String \| Bool | 执行日志路径；`true` = 与程序同名的 `.log`（如 `sf.exe` → `sf.log`），`false` = 不记录。 | `None` |
| `run_as_admin` | Bool | **Windows**：以管理员身份运行目标程序。启用后每次运行会弹出 UAC 提权确认。非 Windows 平台忽略。 | `false` |
| `named_args_mode` | String | 命名参数模式：`all` (解析所有 -k v，从位置参数中移除), `before_sep` (仅解析 `--` 之前的命名参数), `disabled` (禁用命名解析：所有输入全进 $argN),`keep` (复制模式：解析为 $key 变量，但位置参数列表中依然保留一份) | `all` |

---

## 🔍 变量替换规则

在 `args` 模板中，你可以使用以下占位符（下例以默认符号 `$` 演示）：

> 若配置了 `var_symbol = "%"`，把下文的 `$` 换成 `%` 即可（如 `%arg1`、`%{...args}`）；此时 `$` 不再具有特殊含义，会原样传给目标程序。

1.  **位置参数**：
    *   `$arg1`, `$(arg2)`：代表用户输入的第 1, 2... 个非选项参数。
2.  **命名参数**：
    *   `$user`：匹配输入中的 `--user=admin` 或 `-user admin`。
    *   对于开关（如 `-debug`），如果存在，替换值为 `"true"`。
3.  **环境变量**：
    *   `$HOME` / `${PATH}`：匹配系统环境变量。
    *   同时也匹配 TOML 中 `[env]` 部分定义的变量。
4.  **参数数组展开**（区间写法 `[start..end]`，**1 基闭区间**，与 `$argN` 一致，即 `[2..4]` = 第 2~4 个参数；省略界限表示 1 / 末尾，故 `${...args}` 等价于 `${...args[..]}`）：
    *   **多参数模式** `${...args}` / `${...args[2..4]}`：区间内每个参数各自替换，生成多个参数（前后缀会重复）。
        例：模板 `"--${...args[2..4]}x"`、输入 `A1 A2 A3 A4 A5 A6` → `--A2x --A3x --A4x`
    *   **单参数模式** `$(...args)` / `$(...args[2..4])` / `$(...args[2..4],<连接符>)`：先把区间内参数 join 成**一个**参数再整体替换；连接符默认空格，可用 `]` 后的 `,<连接符>` 指定（可为空串或任意文本）。
        *   `"--$(...args[2..4])x"` → `--A2 A3 A4x`
        *   `"--$(...args[2..4],,)x"` → `--A2,A3,A4x`
        *   `"--$(...args[2..4],www)x"` → `--A2wwwA3wwwA4x`

---

## 💡 使用示例

### 场景：封装一个 Fish 脚本调用

假设你有一个程序 `aa.exe`（编译自本项目）和 `aa.toml`。

**aa.toml**
```toml
prog = "C:/msys64/usr/bin/fish.exe"
args = ["-lc", "xxx/yyy/a.fish", "-F=$f", "-d", "$d", "--long2=$long",  "$arg2", "xxx$(arg3)", "$arg1", "args:", "${...args}", "args[1-3]:", "--a-${...args[2..3]}"]
log_file = "C:/logs/aa_exec.log"

[env]
VERSION = "1.0.0"
```

**执行命令：**
```bash
./aa.exe --long=LONG -f -d ddd AA BB CCC DD
```

**实际发生的调用：**
```bash
VERSION=1.0.0 C:/msys64/usr/bin/fish.exe -lc xxx/yyy/a.fish -F=true -d ddd --long2=LONG BB xxxCCC AA args: AA BB CCC DD args[1-3]: --a-BB --a-CCC
```

---

### 场景：包装 Bash 脚本（保留脚本内的 `$var`）

bash/fish 等脚本自身大量使用 `$var`，与默认的 `$` 占位符冲突。把 `var_symbol` 改成 `%` 即可：

**run_sh.toml**
```toml
prog = "C:/msys64/usr/bin/bash.exe"
var_symbol = "%"
args = ["-lc", "%{...args}"]
```

**执行命令：**
```bash
./run_sh.exe 'for i in 1 2 3; do echo $i; done'
```

bash 收到的是原样的 `for i in 1 2 3; do echo $i; done`，`$i` 不会被本工具替换。

---

### 场景：带重定向的 Windows CMD 包装

**task.toml**
```toml
prog = "cmd"
args = ["/C", "dir $path /B > $out"]
shell_quote = false
```

**执行命令：**
```bash
./task.exe --path C:\Users --out list.txt
```

---

### 🛠️ 创建模式（一键生成别名 + 配置）

当与程序同名的 `.toml` 配置文件**不存在**时，工具自动进入创建模式：把自身**复制 / 硬链接 / 符号链接**为别名，并根据命令行选项生成别名对应的 `.toml` 配置（所有 TOML 选项均可转换为命令行参数）。

```text
用法: <本程序> <目标别名路径> [选项]

别名创建方式:
  -l, --hardlink    以硬链接创建别名（需与源在同一磁盘卷）
  -s, --symlink     以符号链接创建别名（Windows 需管理员权限或开发者模式）
  -c, --copy        以复制方式创建别名（默认）

TOML 选项:
  --prog=<程序>           必填，实际执行的程序路径或名称
  --args=<参数>           参数模板，可重复指定
  --var_symbol=<符号>     变量替换前缀符号（默认 "$"）
  --env=<KEY=VALUE>       环境变量，可重复指定
  --shell_quote=<BOOL>
  --stdin / --stdout / --stderr=<模式>   inherit / null / piped
  --log_file=<路径|true>  日志路径；true = 与程序同名的 .log
  --named_args_mode=<模式>
  --run_as_admin=<BOOL>
  --full, --template      非 TOML 项：把全部选项写入配置（未启用的以注释补充）
```

**示例**：创建 `sudo_fish.exe` 硬链接别名，让 fish 以管理员身份运行：

```bash
cmd-wrapper --hardlink C:/msys64/usr/local/bin/sudo_fish.exe \
    --prog=C:/msys64/usr/bin/fish.exe \
    --args=-lc --args='${...args}' \
    --shell_quote=true --run_as_admin=true
```

执行后生成：

```toml
# C:/msys64/usr/local/bin/sudo_fish.toml
prog = "C:/msys64/usr/bin/fish.exe"
args = ["-lc", "${...args}"]
shell_quote = true
run_as_admin = true
```

> 提示：创建模式仅在找不到配置时启用。若你的 `cmd-wrapper.exe` 旁边已有 `cmd-wrapper.toml`，请使用一份不带配置的干净副本作为"安装器"来创建别名。

**`--full` / `--template` 模板**：加上该选项时会生成包含全部选项的配置，未指定的选项以注释形式（带默认值）补充，方便手动编辑：

```toml
prog = "C:/msys64/usr/bin/fish.exe"
args = ["-lc", "${...args}"]

# --- 未启用的选项（默认值，取消注释即可启用）---
# var_symbol = "$"  # 变量替换前缀符号
# env = {}  # 额外环境变量，如 { KEY = "VALUE" }
# shell_quote = false  # 对替换后的变量做引号转义
# stdin = "inherit"  # inherit / null / piped
# stdout = "inherit"  # inherit / null / piped
# stderr = "inherit"  # inherit / null / piped
# log_file = false  # true = 与程序同名的 .log；或写具体路径
# run_as_admin = false  # Windows: 以管理员身份运行
# named_args_mode = "all"  # all / before_sep / disabled / keep
```

---

### 🔐 Windows 管理员权限（UAC 提权）

在 Windows 上，为 TOML 增加 `run_as_admin = true` 后，程序会以**管理员权限**执行目标程序：

1. 启动时先检测当前进程是否已持有管理员（提权）令牌；
2. 若未提权，通过 `runas` verb 触发 **UAC 提权确认框**，并以管理员权限**重新启动自身**（你传入的参数原样传递，含空格/引号的参数不会丢失）；
3. 提权后的实例会**附加到原始终端**（输出/输入仍显示在当前窗口）并**恢复原始工作目录**，然后重新读取同名 `.toml` 配置并正常执行 `prog`；
4. 原始进程等待提权实例结束，并**透传其退出码**；若用户在 UAC 中选择"否"，程序报错退出（退出码 1）。

> 提示：`run_as_admin` 仅对 Windows 生效，其他平台会被忽略并输出警告。提权过程不需要额外安装任何程序，由 Windows 系统 UAC 机制完成。

---

## ⚠️ 注意事项

1.  **权限**：如果配置了 `log_file`，请确保当前用户有权写入该路径。
2.  **Shell 转义**：如果你手动在 `args` 中写了管道符号（如 `|`, `>`），必须将 `shell_quote` 设为 `false`，否则这些符号会被当作普通文本参数传递给程序。
3.  **安全性**：在处理不受信任的用户输入时，请谨慎关闭 `shell_quote` 以防命令注入。
4.  **替换符号**：`var_symbol` 请避开目标脚本自身的语法字符（例如 cmd 用 `%VAR%`、bash/fish 用 `$VAR`），否则容易与脚本里的变量写法混淆。
5.  **UAC 提权**：启用 `run_as_admin = true` 后每次运行都会弹出系统 UAC 确认框（受 Windows 策略限制，程序无法自动绕过）；提权后的目标进程运行在原始终端中，请勿提前关闭原窗口。
```
