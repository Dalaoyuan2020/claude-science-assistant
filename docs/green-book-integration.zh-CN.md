# CSA 与 Claude Science 绿皮书联动方案

本文用于说明 CSA（Claude Science Assistant）和 [Claude Science 绿皮书](https://github.com/Dalaoyuan2020/claude-science-green-book) 如何形成互相支撑的关系。它也可以作为绿皮书第二章的素材草稿。

## 一句话定位

CSA 是绿皮书第二章“装上你的科研搭档”的 Windows 推荐落地工具。

绿皮书负责把读者带进 Claude Science 科研工作流；CSA 负责把 Windows 用户从 WSL、端口、运行时、API Key、模型映射这些环境问题里解放出来。

## 两个仓库各自承担什么

| 仓库 | 主要任务 | 读者心智 |
| --- | --- | --- |
| `claude-science-green-book` | 教读者为什么用、怎么用、怎么把 Claude Science 放进科研流程 | 这是一本实战书 |
| `claude-science-api-bridge` / CSA | 帮读者在 Windows 上装上、启动、接入国产模型并排错 | 这是书里的工具箱 |

不要让两个仓库写成同一份教程。更自然的分工是：

- 绿皮书：用故事、案例、流程讲清楚学习路径。
- CSA：用工程文档讲清楚安装、启动、模型接入、安全与排错。

## 推荐放在绿皮书第二章的位置

绿皮书第二章可以采用这样的结构：

1. 为什么先要装好科研终端。
2. Mac / Linux 用户的基础路径。
3. Windows 用户推荐路径：CSA。
4. 第一次启动 Claude Science。
5. 添加 API Key 与测试模型。
6. 进入第三章：30 分钟摸清一个方向。

CSA 适合放在第 3 节和第 5 节之间。读者不需要先理解完整架构，只需要知道它帮自己跨过 Windows 环境门槛。

## 可以复制到绿皮书第二章的文案

```markdown
### Windows 用户推荐：使用 CSA 装上 Claude Science

如果你是 Windows 用户，我建议优先使用 CSA（Claude Science Assistant）。

Windows 上最容易卡住的地方，不是 Claude Science 本身，而是 WSL2、Ubuntu、端口转发、运行时依赖、API Key 保存和模型名映射。CSA 把这些步骤收口成一个启动器：先检查电脑环境，再安装/修复 WSL 运行时，然后在“添加 API Key”里选择服务商、测试连通、自动映射模型，最后一键启动 Claude Science。

CSA 的目标不是让你学习系统运维，而是让你尽快进入科研工作流。

推荐流程：

1. 到 CSA 仓库下载最新 Release 便携包。
2. 解压后先运行 `1-run-acceptance-preview.bat`，只预览安装计划。
3. 如果电脑已经有可用 WSL/Ubuntu，确认后运行 `4-install-runtime-after-preview.bat`，安装或修复 CSA 在 WSL 内的运行时。
4. 如果电脑还没有 WSL/Ubuntu，不要把它当成静默系统安装器；这一步需要管理员权限、额外确认和可能的重启。新手可以让 Codex 使用包内 Skill 引导完成。
5. 双击 `claude-science-assistant.exe`。
6. 在“添加 API Key”中选择 GLM、LongCat、DeepSeek、MiniMax、OpenCode Go、OpenRouter 或自定义中转。
7. 点击“测试 API Key”和“自动映射”，确认可用后启动 Claude Science。

完成这一步后，你就不用每天重新安装环境了。日常只需要打开 CSA，检查状态，启动 Claude Science，然后继续本书后面的科研任务。

CSA 仓库：https://github.com/Jyx0208/claude-science-api-bridge
```

## CSA README 应该怎样反向链接绿皮书

CSA 首页建议把绿皮书放在靠前位置，而不是藏到最后：

- 在开头说明：CSA 是绿皮书第二章配套工具。
- 在“为什么做 CSA”中解释初衷：帮助读者先用上 Claude Science。
- 在“与绿皮书联动”中给出学习路径表。
- 在 Quick Start 之后提示：跑通后回到绿皮书继续学习科研流程。

这样读者会自然理解：

```text
绿皮书 = 学习路线和科研方法
CSA = Windows 安装、启动、模型接入和排错工具
```

## GitHub 仓库样式建议

CSA 仓库首页不建议一上来就是安装命令。更好的顺序是：

1. Hero：一句话说明它是 Claude Science 的 Windows 启动器与国产模型接入助手。
2. 初衷：为什么要做，解决哪些真实卡点。
3. 原理：Windows 启动器、WSL 运行时、Bridge、Provider 的关系。
4. CC-switch 借鉴点：有序配置、集中添加入口、当前激活项、测试后启用。
5. 绿皮书联动：它是第二章配套工具。
6. 版本能力：v0.1.2 支持什么。
7. Provider 默认顺序与模型映射。
8. 快速开始。
9. 安全边界和排错。
10. 文档导航与 Release 信息。

这个顺序会让 GitHub 首页像一个“产品说明页”，而不是一个临时脚本仓库。

## 读者路径设计

```mermaid
flowchart LR
    A["看到绿皮书"] --> B["第二章：装上科研搭档"]
    B --> C["Windows 用户下载 CSA"]
    C --> D["体检 / 安装 WSL / 启动"]
    D --> E["添加 API Key / 自动映射"]
    E --> F["Claude Science 可用"]
    F --> G["回到绿皮书第三章\n开始科研任务"]
    G --> H["遇到环境问题\n回 CSA 排错"]
```

这个闭环很重要：CSA 不只是一个下载链接，而是绿皮书学习路径的一环。

## 需要避免的写法

- 不要把 CSA 描述成官方 Claude、官方 Claude Science 或官方模型服务。
- 不要承诺“任何电脑都一键成功”。更准确的说法是“先体检，再按计划安装/修复”。
- 不要把第三方中转写得像官方推荐。内置中转只是模板，用户需要确认域名和风险。
- 不要在教程、issue、截图或示例里出现真实 API Key。
- 不要让读者手工复制大段 PowerShell 作为唯一入口；新手优先使用 BAT 和启动器。

## 后续可以做的联动

- 在绿皮书仓库添加 `tools/windows-csa.md`，专门介绍 Windows 路径。
- 在 CSA 仓库 Release 中增加“配套绿皮书章节”链接。
- 在 CSA 启动器里增加“阅读绿皮书下一步”按钮，指向第二章或第三章。
- 在绿皮书第三章开头增加一句：如果 Claude Science 还没启动，先回到 CSA 完成体检与启动。
