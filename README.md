# Cmd-Wrapper

`Cmd-Wrapper` 是一个高度可定制的 Rust CLI 工具，旨在通过简单的 **TOML** 配置文件对程序调用进行“重定向”与“参数重映射”。它可以将复杂的脚本调用隐藏在简单的可执行文件之后，并支持环境变量替换、参数解析及执行日志记录。

## 🚀 核心功能

*   **无感知封装**：根据可执行文件名自动匹配 `.toml` 配置文件。
*   **灵活的参数映射**：支持位置参数 (`$arg1`)、命名参数 (`-k v`) 、开关参数 (`-f`)和参数数组切片展开（`$(...args)` 和 `$(...args[1..3])`）。
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
| `env` | Table | 键值对映射，定义额外的环境变量。 | `{}` |
| `shell_quote` | Bool | 是否自动对替换后的变量进行引号转义。 | `false` |
| `stdin` | String | 标准输入模式: `inherit`, `null`, `piped` | `inherit` |
| `stdout` | String | 标准输出模式: `inherit`, `null`, `piped` | `inherit` |
| `stderr` | String | 标准错误模式: `inherit`, `null`, `piped` | `inherit` |
| `log_file` | String | 执行日志的保存路径。 | `None` |
| `named_args_mode` | String | 命名参数模式：`all` (解析所有 -k v，从位置参数中移除), `before_sep` (仅解析 `--` 之前的命名参数), `disabled` (禁用命名解析：所有输入全进 $argN),`keep` (复制模式：解析为 $key 变量，但位置参数列表中依然保留一份) | `all` |

---

## 🔍 变量替换规则

在 `args` 模板中，你可以使用以下占位符：

1.  **位置参数**：
    *   `$arg1`, `$(arg2)`：代表用户输入的第 1, 2... 个非选项参数。
2.  **命名参数**：
    *   `$user`：匹配输入中的 `--user=admin` 或 `-user admin`。
    *   对于开关（如 `-debug`），如果存在，替换值为 `"true"`。
3.  **环境变量**：
    *   `$HOME` / `${PATH}`：匹配系统环境变量。
    *   同时也匹配 TOML 中 `[env]` 部分定义的变量。

---

## 💡 使用示例

### 场景：封装一个 Fish 脚本调用

假设你有一个程序 `aa.exe`（编译自本项目）和 `aa.toml`。

**aa.toml**
```toml
prog = "C:/msys64/usr/bin/fish.exe"
args = ["-lc", "xxx/yyy/a.fish", "-F=$f", "-d", "$d", "--long2=$long",  "$arg2", "xxx$(arg3)", "$arg1", "args:", "$(...args)", "args[1-3]:", "--a-$(...args[1..3])"]
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

## ⚠️ 注意事项

1.  **权限**：如果配置了 `log_file`，请确保当前用户有权写入该路径。
2.  **Shell 转义**：如果你手动在 `args` 中写了管道符号（如 `|`, `>`），必须将 `shell_quote` 设为 `false`，否则这些符号会被当作普通文本参数传递给程序。
3.  **安全性**：在处理不受信任的用户输入时，请谨慎关闭 `shell_quote` 以防命令注入。
```
