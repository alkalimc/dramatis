# dramatis

把一个协作维基变成有检索依据的角色智能体：一个离线的语料锻造器（forge），加一个运行时引擎（engine）。

forge 抓取维基、把页面归一成结构化记录、打包成单文件知识库；engine 读取这个文件并运行智能体。一切与具体语料相关的东西都在**配置包**（pack）里，本仓库的代码不提及任何作品、角色或站点。

```
dramatis/
  forge/        Python 3.13 —— 离线的一半
    src/dramatis_forge/     机制：harvest · normalize · corpus · evals · report
    tests/                  框架测试，全部不加载任何配置包
  engine/       Rust 2024 —— 运行时
    crates/folio/           读取 .folio：清单、单元、f16 向量、名册、别名
    crates/index/           词法 BM25 ∥ 精确稠密扫描 → RRF 融合 → 置信度
    crates/eval/            评测集运行器与检索指标
    bins/dramatis-cli/      inspect · search · evaluate · bench
  docs/         贡献者文档
```

## 这里只有一半

**本仓库不带任何配置包。** 框架是机制：它知道怎样抓取 MediaWiki 站点、归一成记录、把页面归并为人、切分检索单元、打包单文件语料、构造评测集、度量自身。它不知道*任何具体*站点的事：取哪些页、怎样解析标记、哪个模板声明两页是同一个人、角色该怎样称呼对方、报告用什么语言写。

这些规则就是配置包，写配置包才是真正的工作。把 `DRAMATIS_PACKS` 指向一个包含 `packs/<name>/` 的目录，上面每个阶段就都能用；没有配置包，CLI 无事可做。

契约就是 `forge/src/dramatis_forge/pack.py` 的全部。它刻意很薄：一组声明式规则对象、几个解析函数，挂在模块级的 `PACK` 上。没有插件基类层级——接缝这么窄，那种层级只是装成架构的虚构。

人读的文字同样来自配置包：`Pack.text` 覆盖 `dramatis_forge/text.py` 里的键，缺省值是中性英文。标点、断句位置、停用词也一样，因为它们是语料语言的属性，不是机制。

## 为什么两种语言放在一个仓库

`.folio` 是 Python 写、Rust 读的契约。两边放在一起，格式改动、写方、读方就能在同一个提交里一起变；拆开只会让每次格式调整变成两个仓库之间的来回。

## 上手

```sh
cd forge
make install          # venv + 可编辑安装 + 一个 macOS 路径文件的绕行
make test             # 框架测试
export DRAMATIS_PACKS=/path/to/your/rules   # 包含 packs/<name>/ 的目录
./forge --help
```

从零构建：

```sh
./forge harvest sync --pack <name>     # 唯一联网的阶段；可断点续跑；结束时自动抽样
./forge baseline accept --pack <name>  # 审阅后接受当前计数，作为守卫的期望值
./forge corpus build --pack <name>
./forge evals build --pack <name>
./forge report figures --pack <name>
```

查询产物：

```sh
cd ../engine
cargo build --release
export DRAMATIS_FOLIO=/path/to/<name>.folio
./target/release/dramatis-cli inspect
./target/release/dramatis-cli search "<查询>"
./target/release/dramatis-cli bench     # 延迟分布与常驻内存
```

产物默认落在工作区的 `artifacts/` 下，在所有仓库之外：它们体积大，放进工作树只会让 `git status` 失真。

## 机制与配置的边界

一条规则决定代码放哪：**引擎管机制，配置包管规则。** 一行代码里只要出现维基模板名、角色名、作品术语、站点地址或某种语言的措辞，它就属于配置包，也就不属于本仓库。

度量也在这条缝上。描述语料的文档引用报告里的**键名**而不是数值：抄进散文的数字会活得比产生它的那次构建更久，而下游无从察觉。`report figures` 从制品算出每个键的值，可选的 `target` 由工具逐项判定；`--check` 扫描一个文档树，报告已作废的旧数值和指向不存在目标的引用。

凡是工具能生成的都由工具生成：计数、基线、指纹、日期、由配置推得的 URL。守卫比较的期望计数不写在代码里，而是 `baseline accept` 写出的 `BASELINES.json`。

## 守卫，而不是规则表

规则表的典型失败不是出错，而是**悄悄地**出错：输出照常出现，计数看起来也合理。所以流水线里每条假设都是一个会报警的断言：

| 守卫 | 检查 |
| --- | --- |
| G1 | 种子集漂移（三档），以及产出与入库对账 |
| G2 | 规则没预料到的标记 |
| G3 | 在范围内却什么都没产出的页面 |
| G4 | 页面到人的身份不变量 |
| G5 | 切分的覆盖、尺寸与冗余 |
| G6 | **制品与自身一致**：表 ↔ 清单 ↔ 报告 |

严重度只表达一件事：**内容有没有可能丢了？** 任何高优先发现都会让发布闸门不通过；低优先项必须逐条有归因——没有归因的低优先计数，是尚未被发现的高优先项。

G6 是唯一不检查语料的守卫。一份声称「我没有高优先问题」的文件，需要有东西去核实这句话。

守卫抓得住响亮的失败，抓不住解析**悄悄错了**。所以每次 `harvest sync` 与 `harvest update` 结束时都会重建一份抽样（`report samples`）：每条路由若干随机页、各类边界情况、上次增量里改动与新增的页面，每页三份文件，供人对照着读。

## 许可

Apache-2.0。本仓库**不内嵌任何第三方代码**，只有普通依赖。

它构建出的语料**不**受这份许可覆盖：语料来自源站，带着源站自己的许可，以及其中作品的权利人的权利。再分发时需要逐页署名（带版本号）、遵守源站许可的条件，并给出下架联系方式；`report attribution` 生成的文件就是为此准备的，而且在任何单元缺少来源版本号时拒绝生成。
