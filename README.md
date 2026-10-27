# Cmd-Wrapper

**把一条又长又固定的命令，装进你自己的小 exe 里。**

比如你经常要敲这样一长串：

```bash
C:/msys64/usr/bin/fish.exe -lc C:/scripts/build.fish --target all -v
```

用 Cmd-Wrapper，只要在 `build.exe` 旁边放一个 `build.toml`：

```toml
program = "C:/msys64/usr/bin/fish.exe"
args = ["-lc", "C:/scripts/build.fish", "--target", "$target"]
```

以后就可以直接：

```bash
build.exe --target all
```

它做的事情只有三步：**读同名 TOML → 把你的输入填进模板 → 启动真正的程序**。

---

## 一分钟上手

1. 编译并放到 PATH：

   ```bash
   cargo build --release
   ```

   把 `target/release/cmd-wrapper.exe` 复制成你想要的名字（例如 `build.exe`）。

2. 在它旁边放一个同名配置 `build.toml`（内容见上面的例子）。

3. 运行 `build.exe --target all`。

> **配置文件名怎么定？** exe 文件名 + `.toml`。`build.exe` → `build.toml`，`run-fish.exe` → `run-fish.toml`。

---

## 配置项

只有 `program` 是必填，其它不写就用默认值。

| 字段 | 类型 | 通俗解释 | 默认 |
| :--- | :--- | :--- | :--- |
| `program` | 字符串 | **真正要跑的程序**。 | 必填 |
| `args` | 字符串数组 | **传给程序的参数**，可以混着写死内容和占位符（见下一节）。 | `[]` |
| `env` | 表 | 额外设置的环境变量，只在这次运行生效。 | `{}` |
| `symbol` | 字符串 | 占位符的开头符号。默认 `$`；如果目标脚本自己就大量用 `$变量`，改成 `%` 之类更省心。 | `"$"` |
| `quote_args` | 布尔 | 目标程序会把参数**当命令行再解析**时（例如 `fish -lc "..."`），是否自动给替换进来的内容加引号，避免空格把参数拆开。 | `false` |
| `stdin` / `stdout` / `stderr` | 字符串 | 输入/输出怎么接：`inherit` 接着用当前终端，`null` 丢掉，`piped` 走管道。 | `inherit` |
| `log_file` | 字符串或布尔 | 把每次执行的最终命令记到哪。写路径就写那个文件；写 `true` 自动用同名的 `.log`（`build.exe` → `build.log`）；`false` 不记。 | 不记 |
| `run_as_admin` | 布尔 | **仅 Windows**：以管理员身份运行（每次会弹 UAC 确认框）。 | `false` |
| `options_mode` | 字符串 | 怎么看待输入里 `-名字 值` 这种写法，见「带名字的参数」一节。 | `"parse"` |

> 字段名都按字面意思写：`program` 要跑的程序、`args` 参数、`symbol` 占位符符号、`quote_args` 加引号、`options_mode` 选项解析方式。

---

## 占位符：怎么把输入填进 `args`

`args` 里的字符串可以混着写「固定内容」和「占位符」。下面以默认的 `$` 开头为例。

### 按顺序取参数：`$arg1`、`$arg2`…

「普通参数」指输入里**不以 `-` 开头**的那些。

| 你写 | 输入 | 换进去的值 |
| :--- | :--- | :--- |
| `$arg1` | `A B C` | `A` |
| `$arg2` | `A B C` | `B` |
| `$(arg2)` | `A B C` | `B` |

> `$(arg2)` 是括号写法，紧贴其它文字时更好读，比如 `"xxx$(arg3)"`。

### 带名字的参数：`$名字`

输入里的 `--名字 值`、`--名字=值`、`-名字 值` 都会被认成「带名字的参数」，用 `$名字` 取出来；只有开关没有值（如 `-debug`）时，值是 `true`。

```bash
build.exe --target all -debug
```

```toml
args = ["--target", "$target", "$debug"]   # 实际: --target all true
```

### 环境变量：`$名字`

`$名字` 的查找顺序是：**输入里的同名参数 → `[env]` 里定义的 → 系统环境变量**。所以：

```toml
[env]
VERSION = "1.0.0"
```

```toml
args = ["--version", "$VERSION"]
```

> 注意：环境变量只支持 `$名字`（或 `$(名字)`）这种写法；`${...}` 专门用于下面的参数整串展开。

### 把一串参数整体塞进来

同一个占位符有两种写法，区别只在**塞完之后是几个参数**：

| 写法 | 你可以读成 | 例子（输入 `A1 A2 A3 A4 A5 A6`） |
| :--- | :--- | :--- |
| `${...args}` | 「把这些参数**一个个摊开**」 | `"--${...args[2..4]}x"` → `--A2x --A3x --A4x`（3 个参数） |
| `$(...args)` | 「把这些参数**拼成一个**」 | `"--$(...args[2..4])x"` → `--A2 A3 A4x`（1 个参数） |

**区间怎么写**：`[2..4]` 表示「第 2 个到第 4 个」，**和 `$arg2` 一样从 1 开始数，两头都算**。也可以只写一头或都不写：

| 写法 | 意思 |
| :--- | :--- |
| `${...args[2..4]}` | 第 2~4 个 |
| `${...args[2..]}` | 第 2 个到最后 |
| `${...args[..3]}` | 第 1~3 个 |
| `${...args}` / `${...args[..]}` | 全部（两种写法完全一样） |

**拼成一个时，还能换连接符**（默认是空格）：

| 你写 | 输入 `A1 A2 A3` 时的结果 |
| :--- | :--- |
| `$(...args)` | `A1 A2 A3` |
| `$(...args,,)` | `A1,A2,A3` |
| `$(...args,www)` | `A1wwwA2wwwA3` |
| `$(...args,)` | `A1A2A3`（连接符为空） |
| `$(...args[2..3],,)` | `A2,A3` |

> 小提示：`${...args}` 和 `$(...args)` 只差一个括号/花括号，写混了很容易看不出来。记法：**花括号 `{}` 像展开的列表 → 逐个摊开；圆括号 `()` 像握在一起 → 拼成一个。**

### 换掉开头符号（`symbol`）

如果目标脚本自己就大量使用 `$变量`（bash / fish 基本都是），可以让本工具别抢 `$`：

```toml
symbol = "%"
args = ["-lc", "%{...args}"]
```

这样 `$` 就原样交给脚本，占位符改写成 `%arg1`、`%{...args}`、`%名字` 等。

---

## 带名字的参数怎么解析（`options_mode`）

输入里的 `-名字 值` / `--名字=值` 默认会被认成「带名字的参数」，而且**从普通参数里拿掉**（所以不会再出现在 `$arg1` 里）。想改变这个规则时用：

| 模式 | 你可以读成 | 效果 |
| :--- | :--- | :--- |
| `parse`（默认） | 全都当选项 | `-x 1` 收进 `$x`，不占 `$arg1` 的位置 |
| `before_dashdash` | 只认 `--` 前面的 | `--` 之后的 `-x` 不再当选项，原样留在普通参数里 |
| `off` | 完全不认 | 所有输入都按顺序进 `$arg1`、`$arg2`… |
| `keep` | 既当选项、也留一份 | `-x 1` 收进 `$x`，同时仍占 `$argN` 的位置 |

---

## 常见场景

### 包一个 Fish 脚本

```toml
program = "C:/msys64/usr/bin/fish.exe"
args = ["-lc", "xxx/yyy/a.fish", "-F=$f", "-d", "$d", "--long2=$long", "$arg2", "xxx$(arg3)", "$arg1", "args:", "${...args}", "args[1-3]:", "--a-${...args[2..3]}"]
log_file = "C:/logs/aa_exec.log"

[env]
VERSION = "1.0.0"
```

```bash
./aa.exe --long=LONG -f -d ddd AA BB CCC DD
```

实际执行的是：

```bash
C:/msys64/usr/bin/fish.exe -lc xxx/yyy/a.fish -F=true -d ddd --long2=LONG BB xxxCCC AA args: AA BB CCC DD args[1-3]: --a-BB --a-CCC
```

### 包 Bash 脚本（脚本里的 `$变量` 别被吃掉）

```toml
program = "C:/msys64/usr/bin/bash.exe"
symbol = "%"
args = ["-lc", "%{...args}"]
```

```bash
./run_sh.exe 'for i in 1 2 3; do echo $i; done'
```

bash 收到的是原样的 `for i in 1 2 3; do echo $i; done`，`$i` 不会被本工具替换。

### 包 cmd（要写重定向时）

```toml
program = "cmd"
args = ["/C", "dir $path /B > $out"]
quote_args = false
```

```bash
./task.exe --path C:\Users --out list.txt
```

> 这里必须 `quote_args = false`，否则 `>` 会被加上引号当成普通文字。

---

## 创建模式：不写 toml，一条命令生成

当 exe 旁边**没有**同名 `.toml` 时，程序会进入「创建模式」：把自己复制/硬链接/符号链接成你要的别名，并按命令行参数生成对应的 `.toml`。

```text
用法: <本程序> <目标别名路径> [选项]

别名创建方式:
  -l, --hardlink    硬链接（要求源和目标在同一磁盘卷）
  -s, --symlink     符号链接（Windows 需管理员权限或开发者模式）
  -c, --copy        复制（默认）

TOML 选项:
  --program=<程序>         必填，真正要跑的程序
  --args=<参数>            参数模板，可重复指定，写几次加几个
  --symbol=<符号>          占位符开头符号（默认 "$"）
  --env=<KEY=VALUE>        环境变量，可重复指定
  --quote_args=<BOOL>      是否自动加引号
  --stdin / --stdout / --stderr=<模式>   inherit / null / piped
  --log_file=<路径|true>   日志路径；true = 同名的 .log
  --options_mode=<模式>    带名字参数的解析方式
  --run_as_admin=<BOOL>    Windows：以管理员身份运行
  --full, --template       把全部选项都写进配置（没启用的用注释补齐）
```

**示例**：创建一个 `sudo_fish.exe`，让 fish 以管理员身份跑：

```bash
cmd-wrapper --hardlink C:/msys64/usr/local/bin/sudo_fish.exe \
    --program=C:/msys64/usr/bin/fish.exe \
    --args=-lc --args='${...args}' \
    --quote_args=true --run_as_admin=true
```

生成 `C:/msys64/usr/local/bin/sudo_fish.toml`：

```toml
program = "C:/msys64/usr/bin/fish.exe"
args = ["-lc", "${...args}"]
quote_args = true
run_as_admin = true
```

**`--full` / `--template`**：把没启用的选项也写进配置（以注释形式给出默认值），方便以后手动改：

```toml
program = "C:/msys64/usr/bin/fish.exe"
args = ["-lc", "${...args}"]

# --- 未启用的选项（默认值，取消注释即可启用）---
# symbol = "$"  # 占位符开头符号
# env = {}  # 额外环境变量，如 { KEY = "VALUE" }
# quote_args = false  # 自动给替换内容加引号
# stdin = "inherit"  # inherit / null / piped
# stdout = "inherit"  # inherit / null / piped
# stderr = "inherit"  # inherit / null / piped
# log_file = false  # true = 同名的 .log；或写具体路径
# run_as_admin = false  # Windows: 以管理员身份运行
# options_mode = "parse"  # parse / before_dashdash / off / keep
```

> 提示：创建模式只在「找不到配置」时触发。如果你的 `cmd-wrapper.exe` 旁边已经有 `cmd-wrapper.toml`，请改用一份没有配置的干净副本当「安装器」来创建别名。

---

## Windows 管理员权限（UAC）

在 Windows 上，给 TOML 加上 `run_as_admin = true`，程序就会以管理员身份运行目标程序：

1. 先看自己是不是已经是管理员；
2. 不是的话，弹出 **UAC 确认框**，确认后以管理员身份**重新启动自己**（你传的参数会原样带过去，空格和引号都不会丢）；
3. 新实例会**接回原来的终端**（输出还在当前窗口）、恢复原来的工作目录，然后照常执行 `program`；
4. 原进程等它结束，并把它的退出码原样返回；如果你在 UAC 里点了「否」，程序以退出码 1 结束。

> 只在 Windows 生效，其它平台会忽略并给出警告。整个过程由系统 UAC 完成，不需要额外装任何东西。

---

## 注意事项

1. **写日志要权限**：`log_file` 指向的目录要能写。
2. **重定向就用 `quote_args = false`**：`args` 里手写了 `|`、`>` 等符号时必须关掉，否则会被当成普通文字。
3. **安全**：处理不可信输入时，谨慎关闭 `quote_args`，以免命令注入。
4. **换符号要避开冲突**：`symbol` 别选目标脚本自己也在用的符号（cmd 用 `%VAR%`、bash/fish 用 `$VAR`）。
5. **UAC 每次都弹**：`run_as_admin = true` 时每次运行都会弹确认框（Windows 策略限制）；别提前关掉原来的终端窗口。
