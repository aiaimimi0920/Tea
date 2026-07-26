// Tea desktop internationalization.
//
// Lightweight i18n with no runtime dependency: a flat dictionary keyed by the
// original English source string maps to the localized string. `t(en)` returns
// the active-locale translation, falling back to the English source when no
// translation exists (so a missing key degrades gracefully instead of showing a
// blank or a raw key). This keeps call sites readable — `t("Approve")` — and lets
// the English source double as the key.
//
// Default locale is Chinese ("zh") per product requirement; an operator can
// toggle to English at runtime and the choice persists in localStorage.

export type Locale = "zh" | "en";

const LOCALE_STORAGE_KEY = "tea.locale";

let activeLocale: Locale = "zh";

const listeners = new Set<(locale: Locale) => void>();

// Chinese translations keyed by English source string. When a string is not
// present here, `t()` returns the English source unchanged.
const zh: Record<string, string> = {
  // App shell / header
  "Tea": "Tea",
  "AI Work Orders": "AI 工单",
  "Tea daemon": "Tea 守护进程",
  "Gitea-style local issue tracker for AI tickets, approvals, runs, and evidence.":
    "面向 AI 工单、审批、执行与证据的 Gitea 风格本地工单追踪器。",
  "Activity log": "活动日志",
  "Daemon status": "守护进程状态",
  "Daemon mode": "守护进程模式",
  "Store backend": "存储后端",
  "Configuration source": "配置来源",
  "Connecting to Tea daemon...": "正在连接 Tea 守护进程……",
  "Tea daemon connected": "Tea 守护进程已连接",
  "Tea daemon is not fully ready": "Tea 守护进程尚未就绪",

  // Queue / list controls
  "Current queue": "当前队列",
  "Custom filtered queue": "自定义筛选队列",
  "A custom combination of state, signal, search, author, label, priority, and risk filters.": "状态、信号、搜索、作者、标签、优先级和风险筛选的自定义组合。",
  "Signal": "信号",
  "Signal summary": "信号概览",
  "Sort": "排序",
  "Recently updated": "最近更新",
  "Newest created": "最新创建",
  "Most activity": "最多活动",
  "Latest touch": "最近变更",
  "Density": "密度",
  "Comfortable": "宽松",
  "Compact": "紧凑",
  "High priority": "高优先级",
  "High risk": "高风险",
  "Watched": "已关注",
  "Active filters": "启用的筛选",
  "Author filters": "作者筛选",
  "Label filters": "标签筛选",
  "Saved view": "已保存视图",
  "Search issues...": "搜索工单……",
  "All signals": "全部信号",

  // Signal labels
  "Active": "进行中",
  "Needs review": "待审阅",
  "Queued": "排队中",
  "Resolved": "已解决",
  "Stale": "陈旧",

  // New work order form
  "New Work Order": "新建工单",
  "New work order metadata": "新建工单元数据",
  "Create an AI issue with a goal, context, and acceptance criteria.":
    "创建一个带有目标、上下文与验收标准的 AI 工单。",
  "Title": "标题",
  "Description": "描述",
  "Priority": "优先级",
  "Labels": "标签",
  "Initial labels": "初始标签",
  "Default approval policy": "默认审批策略",
  "Tea work-order description": "Tea 工单描述",
  "Work order title": "工单标题",
  "Work order description": "工单描述",
  "Title: e.g. Investigate provider failure in login flow":
    "标题：例如 排查登录流程中的服务商故障",
  "Write a markdown description: background, expected result, constraints, and evidence needed.":
    "用 Markdown 编写描述：背景、预期结果、约束以及所需证据。",
  "Comma-separated, e.g. area:auth, needs-triage": "逗号分隔，例如 area:auth, needs-triage",
  "comma-separated operator labels": "逗号分隔的操作员标签",
  "e.g. high, normal, low": "例如 high、normal、low",
  "Suggested work-order templates": "建议的工单模板",

  // Priority options
  "Default priority (normal)": "默认优先级（普通）",
  "Low": "低",
  "Normal": "普通",
  "High": "高",
  "Urgent": "紧急",

  // Approval policy options
  "Plan only": "仅计划",
  "Human before execute": "执行前需人工确认",
  "Human before write": "写入前需人工确认",
  "Human before external network": "外部网络访问前需人工确认",
  "Human before destructive action": "破坏性操作前需人工确认",
  "Human before completion": "完成前需人工确认",
  "Auto if low risk": "低风险时自动",
  "Auto if validation passes": "校验通过时自动",
  "Manual only": "仅手动",
  "Always auto": "始终自动",
  "Select approval policy": "选择审批策略",

  // Actions
  "Analyze": "分析",
  "Analyzed": "已分析",
  "Plan": "计划",
  "Decompose": "拆解",
  "Approve": "批准",
  "Reject": "驳回",
  "Run": "执行",
  "Accept": "验收",
  "Close": "关闭",
  "Cancel": "取消",
  "Cancel edit": "取消编辑",
  "Stop run": "停止执行",
  "Retry run": "重试执行",
  "Retry planning": "重试计划",
  "Quick actions": "快捷操作",
  "Workflow actions": "工作流操作",
  "Edit issue": "编辑工单",
  "Watch": "关注",
  "Watch issue": "关注工单",
  "Watching issue": "正在关注工单",
  "Watched locally": "已本地关注",
  "Preview audit export": "预览审计导出",

  // Detail / sections
  "Issue / Work Order": "工单",
  "Details": "详情",
  "Status": "状态",
  "Source": "来源",
  "Owner": "负责人",
  "Assignee": "指派给",
  "Agent": "智能体",
  "Me": "我",
  "Risk": "风险",
  "Policy": "策略",
  "Approval policy": "审批策略",
  "Review and routing": "审阅与路由",
  "Connection and ownership": "连接与归属",
  "Current connection": "当前连接",
  "Current stage": "当前阶段",
  "Conversation": "对话",
  "Conversation timeline": "对话时间线",
  "Comments": "评论",
  "Comments and daemon events": "评论与守护进程事件",
  "Daemon events": "守护进程事件",
  "Analysis": "分析",
  "AI analysis and plan": "AI 分析与计划",
  "Acceptance criteria": "验收标准",
  "Constraints": "约束",
  "Missing context": "缺失的上下文",
  "Required tools": "所需工具",
  "Target components": "目标组件",
  "Target paths": "目标路径",
  "Expected artifacts": "预期产物",
  "Rollback strategy": "回滚策略",
  "Validation strategy": "校验策略",
  "Run history": "执行历史",
  "Run records": "执行记录",
  "Runs": "执行",
  "Issues": "工单",
  "Author": "作者",
  "Label": "标签",
  "Execution progress": "执行进度",
  "Export": "导出",
  "Export preview": "导出预览",
  "Export this work order": "导出该工单",
  "Preview JSON": "预览 JSON",
  "Preview JSON export": "预览 JSON 导出",
  "Preview Markdown": "预览 Markdown",
  "Preview Markdown export": "预览 Markdown 导出",
  "Configuration JSON": "配置 JSON",
  "Raw ticket JSON": "原始工单 JSON",
  "JSON / Markdown": "JSON / Markdown",
  "Idle": "空闲",

  // Comments
  "Leave a review comment": "留下审阅评论",
  "Add a review comment or run an action to populate the timeline.":
    "添加审阅评论或执行操作以填充时间线。",
  "Comments are durable and included in JSON/Markdown exports.":
    "评论会被持久保存，并包含在 JSON/Markdown 导出中。",
  "Comment editor mode": "评论编辑器模式",
  "Comment": "评论",
  "Reject with reason": "附理由驳回",
  "Reject reason is required": "驳回理由为必填项",
  "Explain why this approval is rejected.": "请说明驳回该审批的原因。",

  // Local notes
  "Local notes": "本地备注",
  "Local notes editor": "本地备注编辑器",
  "Local notes in header": "标题栏中的本地备注",
  "Add note": "添加备注",
  "Hide local notes": "隐藏本地备注",
  "Add a local note": "添加一条本地备注",
  "No local notes yet. Add a private note for this ticket.":
    "暂无本地备注。为该工单添加一条私有备注。",
  "Cleared local notes for this work order": "已清除该工单的本地备注",

  // Empty / placeholder states
  "No labels": "暂无标签",
  "No analysis yet.": "暂无分析。",
  "No comments or events yet.": "暂无评论或事件。",
  "No conversation yet.": "暂无对话。",
  "No description was provided.": "未提供描述。",
  "No plan steps recorded.": "未记录计划步骤。",
  "No runs yet.": "暂无执行记录。",
  "No operator actions recorded yet.": "暂无已记录的操作员动作。",
  "No timeline entries match this filter.": "没有符合该筛选的时间线条目。",
  "No watched work orders match this queue.": "没有符合该队列的已关注工单。",
  "No work orders match this filter.": "没有符合该筛选的工单。",
  "No authors available for filtering.": "暂无可筛选的作者。",
  "No labels available for filtering.": "暂无可筛选的标签。",
  "No extra filters; showing the default open queue.": "无额外筛选；显示默认的未关闭队列。",
  "None recorded": "无记录",
  "Nothing to preview yet.": "暂无可预览内容。",
  "Activity loading": "活动加载中",

  // Settings
  "Tea local settings": "Tea 本地设置",
  "Tea owns these settings until Loom claims Tea configuration.":
    "在 Loom 接管 Tea 配置之前，这些设置由 Tea 自行管理。",
  "Loom manages Tea configuration.": "Loom 正在管理 Tea 配置。",
  "Enable notifications": "启用通知",
  "Human ticket default approval policy": "人工工单默认审批策略",
  "Hook ticket default approval policy": "Hook 工单默认审批策略",
  "Local AI operator": "本地 AI 操作员",

  // Longer descriptive copy
  "Select a work order from the list, or create a new one to start an AI task.":
    "从列表中选择一个工单，或新建一个以启动 AI 任务。",
  "Structured decomposition produced by the Tea BrainProvider or Loom for this work order.":
    "由 Tea BrainProvider 或 Loom 为该工单生成的结构化拆解。",
  "Use the Review actions to analyze or decompose this work order into a plan.":
    "使用「审阅」操作来分析或将该工单拆解为计划。",
  "Approve and launch an AI task to see execution attempts here.":
    "批准并启动 AI 任务后，可在此查看执行尝试。",
  "Inspect execution attempts, statuses, and evidence returned from Loom or fallback runners.":
    "查看来自 Loom 或后备执行器的执行尝试、状态与证据。",
  "Generate portable JSON or Markdown evidence, including comments, events, runs, and ticket state.":
    "生成可移植的 JSON 或 Markdown 证据，包含评论、事件、执行与工单状态。",
  "Use exports to hand off this work order to logs, reviews, or external systems.":
    "使用导出将该工单移交至日志、评审或外部系统。",
  "Review the durable human comments and Tea daemon events for this work order.":
    "查看该工单的持久化人工评论与 Tea 守护进程事件。",
  "Human review comments and daemon state changes in one issue thread.":
    "在同一工单会话中呈现人工审阅评论与守护进程状态变更。",
  "Verify which local Tea daemon and configuration surface currently own this work order view.":
    "确认当前由哪个本地 Tea 守护进程与配置界面掌管该工单视图。",
  "Watch a work order locally, or show all work orders for the current filters.":
    "在本地关注某个工单，或显示当前筛选下的全部工单。",
  "Changing the policy retightens or relaxes the run gate for this work order.":
    "调整策略会收紧或放宽该工单的执行门槛。",
  "Closed and cancelled tickets are read-only.": "已关闭和已取消的工单为只读。",
  "Closed records ready for audit/export.": "可供审计/导出的已关闭记录。",
  "Planning and review are in progress.": "计划与审阅进行中。",

  // Event kind labels (timeline)
  "Ticket created": "工单已创建",
  "Comment added": "已添加评论",
  "Plan proposed": "已提出计划",
  "Plan ready": "计划就绪",
  "Run queued": "执行已排队",
  "Run started": "执行已开始",
  "Run event received": "已收到执行事件",
  "Run failed": "执行失败",
  "Run succeeded": "执行成功",
  "System event": "系统事件",
  "System event batch": "系统事件批次",
  "Show event payload": "显示事件负载",

  // Section aria labels
  "Tea work-order sections": "Tea 工单区块",
  "Issue queue navigation": "工单队列导航",
  "Work order index": "工单索引",
  "Search work orders": "搜索工单",
  "Sort work orders": "工单排序",
  "Filter work orders by signal": "按信号筛选工单",
  "Signal queue filters": "信号队列筛选",
  "Priority and risk quick filters": "优先级与风险快捷筛选",
  "Preset work-order queues": "预设工单队列",
  "Active issue filters": "启用的工单筛选",
  "Issue state filters": "工单状态筛选",
  "Issue list density": "工单列表密度",
  "Current queue summary": "当前队列概览",
  "Issue metadata summary": "工单元数据概览",
  "Issue routing context": "工单路由上下文",
  "Issue labels in header": "标题栏中的工单标签",
  "Timeline activity summary": "时间线活动概览",
  "Timeline filters": "时间线筛选",
  "Conversation timeline ": "对话时间线",
  "Focused analysis and plan section": "分析与计划专注区",
  "Focused comments section": "评论专注区",
  "Focused export section": "导出专注区",
  "Focused runs section": "执行专注区",
  "Focused settings section": "设置专注区",
  "Milestone progress": "里程碑进度",
  "New work order metadata ": "新建工单元数据",
  "Ticket analysis": "工单分析",
  "Ticket plan": "工单计划",

  // Language toggle
  "Language": "语言",
  "Chinese": "中文",
  "English": "English",

  // Filter + list controls
  "Filters": "筛选",
  "Clear filters": "清除筛选",
  "Clear all filters": "清除全部筛选",
  "Open": "待处理",
  "Closed": "已关闭",
  "All": "全部",
  "Reset view": "重置视图",

  // Navigation tabs
  "Exports": "导出",
  "Settings": "设置",

  // Work order templates
  "Investigation": "调查",
  "Implementation": "实现",
  "Release validation": "发布验证",

  // Edit + comment form
  "Save changes": "保存修改",
  "Write": "编辑",
  "Preview comment": "预览评论",
  "Copy JSON": "复制 JSON",
  "Copy Markdown": "复制 Markdown",
  "Download .json": "下载 .json",
  "Download .md": "下载 .md",
  "Download JSON": "下载 JSON",
  "Download Markdown": "下载 Markdown",
  "Download JSON export": "下载 JSON 导出",
  "Download Markdown export": "下载 Markdown 导出",

  // Preset queue labels
  "Default open queue": "默认待办队列",
  "Review queue": "待审阅队列",
  "Stale queue": "陈旧队列",
  "Active work": "进行中",
  "Queued intake": "待接收",
  "Resolved audit": "已归档审计",

  // Preset queue descriptions
  "Default inbox for unfinished work orders.": "未完成工单的默认收件箱。",
  "Risky, high-priority, or noisy work orders.": "高风险、高优先级或噪声较多的工单。",
  "Open work orders without recent touch.": "长时间未更新的待处理工单。",
  "Open work orders with recent activity.": "近期有活动的待处理工单。",
  "Waiting for first analysis or routing.": "等待首次分析或分派。",

  // Toast notifications
  "Reset issue view preferences": "已重置工单视图偏好",
  "Approval rejected": "已拒绝审批",
  "Work order title is required": "必须填写工单标题",
  "No changes to save": "没有可保存的更改",
  "Work order updated": "工单已更新",
  "Tea local configuration saved": "已保存 Tea 本地配置",
  "Review comment cannot be empty": "评论内容不能为空",
  "Review comment added": "已添加评论",
  "Note text is required": "必须填写备注内容",
  "Title must be at least 3 characters": "标题至少需要 3 个字符",
  "Title must be at most 200 characters": "标题最多 200 个字符",
  "Description must be at least 10 characters": "描述至少需要 10 个字符",
  "Creating...": "创建中...",
  "Created work order": "已创建工单",
  "Create failed": "创建失败",
  "Copied work order dep link": "已复制工单链接",
  "Exported work order as JSON": "已导出工单为 JSON",
  "Exported work order as Markdown": "已导出工单为 Markdown",

  // Detail panel + navigation controls
  "Leave blank to use TEA_AUTH_TOKEN/dev-token": "留空则使用 TEA_AUTH_TOKEN/dev-token",
  "Clear log": "清除日志",
  "Clear author": "清除作者筛选",
  "Filtering by author:": "按作者筛选：",
  "Clear filter": "清除筛选",
  "Filtering by label:": "按标签筛选：",
  "Submit work order": "提交工单",
  "Show all work orders": "显示全部工单",
  "Show more work orders": "显示更多工单",
  "Collapse list": "收起列表",
  "Previous issue": "上一个工单",
  "Alt+ArrowUp / Alt+ArrowDown · Alt+Home / Alt+End": "Alt+↑ / Alt+↓ · Alt+Home / Alt+End",
  "Select first matching issue": "选择第一个匹配工单",
  "Next issue": "下一个工单",
  "Copy issue link": "复制工单链接",
  "Reject approval": "拒绝审批",
  "Remove note": "删除备注",
  "Clear notes": "清除备注",
  "Save Tea settings": "保存 Tea 设置",
  "Reset changes": "重置更改",
  "Intent:": "意图：",
  "Recommended workflow:": "推荐工作流：",
  "Open Loom Tea settings": "打开 Loom Tea 设置",
  "Loom did not provide a Tea configuration panel URL.": "Loom 未提供 Tea 配置面板 URL。",
  "Events": "事件",
  "Consecutive low-level daemon events folded to keep the work-order discussion readable.":
    "连续的底层守护进程事件已折叠，以保持工单讨论的可读性。",
  "Run actions are disabled for terminal work orders.": "终态工单已禁用执行操作。",

  // List row + detail routing context
  "Tea local operator": "Tea 本地操作员",
  "not delegated": "未分派",
  "opened": "创建于",
  "comments": "条评论",
  "runs": "次执行",
  "Work Order": "工单",
  "default policy": "默认策略",
  "normal": "普通",
  "medium": "中等",
  "high": "高",
  "low": "低",
  "critical": "紧急",
  "desktop": "桌面端",

  // Header refresh controls
  "Refresh": "刷新",
  "Refreshing...": "刷新中……",
  "Auto-refresh on": "自动刷新：开",
  "Auto-refresh off": "自动刷新：关",
  "Updated": "更新于",
  "Not refreshed yet": "尚未刷新",

  // Count summary + queue labels
  "all work orders": "全部工单",
  "open work orders": "待处理工单",
  "closed work orders": "已关闭工单",
  "Showing": "显示",
  "of": "/",
  "open": "待处理",
  "closed": "已关闭",
  "Signal focus:": "信号焦点：",
  "work orders in the current queue.": "个工单（当前队列）。",
  "Selected outside current queue": "所选工单不在当前队列",
  "Queue position": "队列位置",

  // Detail status + timeline
  "Latest AI action": "最新 AI 操作",
  "Latest system event": "最新系统事件",
  "Not watched": "未关注",
  "approval required": "需要审批",
  "no gate": "无门禁",
  "Not analyzed": "尚未分析",
  "Run Analyze or Decompose from the workflow actions to generate these records. They persist in Tea and are included in JSON/Markdown exports.":
    "从工作流操作中运行“分析”或“拆解”以生成这些记录。它们会保存在 Tea 中，并包含在 JSON/Markdown 导出内。",
  "Copied entry link": "已复制条目链接",
  "Copy entry link": "复制条目链接",
  "Daemon event": "守护进程事件",
  "terminal ticket": "终态工单",
  "markdown supported": "支持 Markdown",

  // Settings connection panel
  "Bearer token": "Bearer 令牌",
  "(launcher/env configured)": "（由启动器/环境变量配置）",
  "(optional override)": "（可选覆盖）",
  "Tea daemon connected.": "Tea 守护进程已连接。",
  "Tea daemon offline or health-only.": "Tea 守护进程离线或仅健康检查。",
  "HTTP API online": "HTTP API 在线",
  "Health endpoint only": "仅健康检查端点",
  "Offline": "离线",

  // Workflow action group titles
  "Review actions": "评审操作",
  "Approval actions": "审批操作",
  "Execution actions": "执行操作",
  "Resolution actions": "结项操作",

  // Signal action hints
  "Export audit record": "导出审计记录",
  "Inspect latest run": "查看最新执行",
  "Inspect repeated runs": "查看重复执行",
  "Monitor active work": "监控进行中的工作",
  "Prioritize review": "优先评审",
  "Review conversation": "查看对话",
  "Review risk before run": "执行前评估风险",
  "Ping owner or retry planning": "提醒负责人或重试计划",
  "Start analysis": "开始分析",
  "Open conversation": "打开对话",
  "Open review thread": "打开评审讨论",
  "Open runs tab": "打开执行记录",
  "Latest human review": "最新人工评审",

  // New work order templates
  "Investigate AI workflow failure": "排查 AI 工作流故障",
  "Implement AI work-order change": "实现 AI 工单变更",
  "Prepare Tea release validation": "准备 Tea 发布验证",

  // Queue summary labels
  "Matching": "匹配",
  "Visible": "可见",
  "Extra filters": "额外筛选",
  "State": "状态",
  "Search": "搜索",

  // Signal reason strings (interpolated with {placeholders})
  "Terminal state: {status}": "终态：{status}",
  "High priority: {value}": "高优先级：{value}",
  "High risk: {value}": "高风险：{value}",
  "Repeated runs: {count}": "重复执行：{count} 次",
  "No touch for {h}h": "已 {h} 小时无操作",
  "Recent activity: touched {h}h ago": "近期活动：{h} 小时前有操作",
  "Recent activity: {count} runs": "近期活动：{count} 次执行",
  "Recent activity: {count} comments": "近期活动：{count} 条评论",
  "Waiting for first touch": "等待首次操作",
  "priority flag": "优先级标记",
  "risk flag": "风险标记",

  // Event kind labels (daemon timeline)
  "Event": "事件",
  "Event payload": "事件负载",
  "Ticket analyzed": "工单已分析",
  "Policy updated": "策略已更新",
  "Ticket edited": "工单已编辑",
  "Approval requested": "已请求审批",
  "Approval granted": "已批准",
  "Evidence attached": "已附加证据",
  "Review requested": "已请求评审",
  "Human accepted": "人工已验收",
  "Ticket closed": "工单已关闭",
  "Ticket cancelled": "工单已取消",

  // Conversation stream meta
  "daemon events": "条守护进程事件",
  "review comment": "评审评论",
  "daemon event": "守护进程事件",
  "{c} review comments, {e} timeline events, {r} run records.":
    "{c} 条评审评论、{e} 条时间线事件、{r} 条执行记录。",
  "Comments are durable review records; daemon events explain how Tea moved the issue through analysis, approval, and execution.":
    "评论是持久的评审记录；守护进程事件说明 Tea 如何推动工单经过分析、审批与执行。",

  // Conversation group labels + summaries
  "Review comment": "评审评论",
  "AI action": "AI 操作",
  "Human review note attached to this work order.": "附加到该工单的人工评审备注。",
  "{group} recorded by {actor}.": "{group}，由 {actor} 记录。",
  "No recent human or daemon touches.": "近期没有人工或守护进程操作。",

  // Stage + milestone prose
  "Default view": "默认视图",
  "This work order reached a terminal state.": "该工单已进入终态。",
  "Execution evidence is already present for operator review.": "已存在可供操作员评审的执行证据。",
  "Human review exists and the work order is waiting on the next action.": "已有人工评审，工单正在等待下一步操作。",
  "Tea has already started planning or routing the work order.": "Tea 已开始为该工单进行计划或分派。",
  "The work order is waiting for initial analysis or approval.": "该工单正在等待首次分析或审批。",
  "default": "默认",
  "Terminal issue state reached.": "已进入终态。",
  "Execution evidence is available.": "已有执行证据。",
  "Human review is captured in comments.": "评论中已记录人工评审。",
  "Waiting for analysis or approval.": "等待分析或审批。",
  "events": "条事件",
  "entries": "条记录",
  "to": "至",

  // Queue summary values
  "Most activity first": "活动最多优先",
  "Newest created first": "最新创建优先",
  "Latest touch first": "最近变更优先",
  "Recently updated first": "最近更新优先",
  "Compact rows · {n} per page": "紧凑行 · 每页 {n} 条",
  "Comfortable rows · {n} per page": "宽松行 · 每页 {n} 条",
  "{n} work orders": "{n} 个工单",
  "{n} in view": "已显示 {n} 个",
  "{n} active": "{n} 项启用",
  "None": "无",

  // Daemon status + analysis badges + section hints
  "Loading": "加载中",
  "Online": "在线",
  "Health-only": "仅健康检查",
  "edited": "编辑于",
  "risk:": "风险：",
  "unknown": "未知",
  "confidence:": "置信度：",
  "policy:": "策略：",
  "No ticket selected.": "未选择工单。",
  "Run cards show execution attempts and evidence. Stop/retry actions stay tied to the selected work order.":
    "执行卡片展示执行尝试与证据。停止/重试操作始终与所选工单绑定。",
  "Use JSON for machine review and Markdown for human handoff, incident notes, or release evidence.":
    "JSON 适用于机器审阅，Markdown 适用于人工交接、事故记录或发布证据。",
  "This panel summarizes the local daemon connection and where Tea-specific configuration should be owned.":
    "此面板汇总本地守护进程连接情况，以及 Tea 专属配置应归属的位置。",
};

const dictionaries: Record<Locale, Record<string, string>> = {
  zh,
  en: {},
};

function readStoredLocale(): Locale {
  try {
    if (typeof localStorage === "undefined") return "zh";
    const stored = localStorage.getItem(LOCALE_STORAGE_KEY);
    return stored === "en" || stored === "zh" ? stored : "zh";
  } catch {
    return "zh";
  }
}

activeLocale = readStoredLocale();

export function getLocale(): Locale {
  return activeLocale;
}

export function setLocale(locale: Locale): void {
  activeLocale = locale;
  try {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(LOCALE_STORAGE_KEY, locale);
    }
  } catch {
    // ignore persistence failures (private mode, etc.)
  }
  for (const listener of listeners) {
    listener(locale);
  }
}

export function subscribeLocale(listener: (locale: Locale) => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// Translate an English source string to the active locale, falling back to the
// source string when no translation exists.
export function t(source: string): string {
  const table = dictionaries[activeLocale];
  const hit = table[source];
  return hit != null && hit !== "" ? hit : source;
}

// React hook: returns the active locale and re-renders the calling component
// whenever the locale changes (via setLocale). Import React lazily through the
// standard hooks so this module stays dependency-light.
import { useEffect as reactUseEffect, useState as reactUseState } from "react";

export function useLocale(): Locale {
  const [locale, setLocaleState] = reactUseState<Locale>(activeLocale);
  reactUseEffect(() => subscribeLocale(setLocaleState), []);
  return locale;
}
