/**
 * zh-CN dictionary. 交互设计 6.2 的固定文案逐字收录，不得改写。
 *
 * 键名按界面分区组织；`{name}` 形式的占位符由 `t()` 替换。
 */
export const zhCN = {
  brand: {
    name: "CC避风港",
    latin: "CCHaven",
  },

  common: {
    cancel: "取消",
    retry: "重试",
    back: "上一步",
    next: "下一步",
    confirm: "确定",
    close: "关闭",
    delete: "删除",
    edit: "编辑",
    undo: "撤销",
    refresh: "刷新",
    copyPath: "复制路径",
    revealInFinder: "在 Finder 中显示",
    openWithEditor: "用编辑器打开",
    loading: "加载中…",
    copied: "已复制路径。",
  },

  // 6.2 防枚举与限频文案（安全语义固定，不得随意改写）
  fixed: {
    sessionExpired: "登录已过期，请重新登录。",
    trialReuse: "每个账号只可享用一次免费试用。",
  },

  login: {
    title: "登录",
    button: "通过浏览器登录 ↗",
    explain:
      "将打开系统浏览器跳转到 cchaven.cn。没有账号？在浏览器里可以直接注册。应用本身不收集你的密码。",
    waitingTitle: "等待浏览器授权…",
    waitingBody: "已在浏览器中打开登录页。完成登录并点击「授权」后，这里会自动进入。",
    reopen: "重新打开浏览器",
    failedTitle: "授权未完成",
    timeout: "等待授权超时。浏览器可能没有打开，或你尚未在浏览器中完成登录。",
    manualHint: "一直失败？可以把浏览器中显示的授权码粘贴到这里：",
    manualPlaceholder: "粘贴授权码",
    manualSubmit: "使用授权码登录",
    offlineError: "无法连接服务器。",
    offlineEnter: "离线使用",
    trialGranted: "🎁 首月免费试用已开通，有效期至 {date}。",
  },

  offline: {
    banner: "当前处于离线只读模式，终端与同步已暂停。",
    retry: "重试连接",
  },

  sidebar: {
    projects: "项目",
    newProject: "+ 新建项目",
    empty: "还没有项目。",
    editProject: "编辑…",
    deleteProject: "删除…",
    recentActivity: "最近活动",
    noActivity: "暂无同步活动。",
  },

  status: {
    synced: "已全部同步",
    syncing: "正在同步 {n} 个文件…",
    conflicts: "{n} 个冲突",
    offline: "离线 — {n} 秒后重试",
    reconnected: "已恢复连接，继续同步…",
  },

  emptyState: {
    title: "把你的云服务器变成 Claude Code 工作台",
    body: "只需要服务器的 IP 地址和密码，3 分钟完成设置。没有服务器也没关系，向导里有购买教程。",
    action: "+ 新建项目",
  },

  deleteProject: {
    title: "移除项目「{name}」？",
    body: "该项目将从 CC避风港 中移除。不会删除任何本地或远端文件。",
    confirm: "移除项目",
    done: "已移除项目「{name}」。",
  },

  wizard: {
    createTitle: "新建项目",
    editTitle: "编辑项目",
    steps: ["连接服务器", "项目设置", "完成"],

    helper:
      "在阿里云、腾讯云、AWS 等平台买好服务器后，你会得到一个 IP 地址和 root 密码，填到下面就行，其余都交给我们。",
    helperLink: "还没有服务器？看购买教程 ↗",

    addressLabel: "服务器 IP 地址",
    addressPlaceholder: "例如 43.156.20.8",
    addressHint: "支持直接粘贴整条 ssh 命令（如 ssh root@43.156.20.8），会自动识别。",
    userLabel: "用户名",
    userHint: "新买的服务器一般就是 root。",
    passwordLabel: "密码",
    passwordPlaceholder: "买服务器时设置的密码",
    passwordHint: "密码只保存在你自己的电脑上（系统钥匙串加密）。",
    pasteDetected: "已自动识别服务器地址和用户名。",
    pasteDetectedHostOnly: "已自动识别服务器地址。",

    advanced: "高级选项（懂 SSH 的用户使用）",
    portLabel: "端口",
    authLabel: "认证方式",
    authPassword: "密码（默认）",
    authKey: "SSH 密钥",
    authSshConfig: "从 ~/.ssh/config 选择主机",
    keyPathLabel: "私钥路径",
    sshConfigLabel: "已有主机",

    connectAndContinue: "连接并继续",
    connecting: "正在连接…",
    connectOk: "✓ 连接成功！服务器环境正常（{distro}），正在进入下一步…",
    connectFailTitle: "连不上服务器，请按顺序检查：",
    connectFail1: "IP 地址是否照着云平台控制台抄对了（公网 IP，不是内网 IP）；",
    connectFail2: "密码是否正确（注意大小写，建议重新复制粘贴）；",
    connectFail3: "云平台的「安全组 / 防火墙」是否放行了 {port} 端口。",
    connectFailLink: "查看图文排查教程 ↗",
    needAddress: "请填写服务器 IP 地址。",
    needPassword: "请填写密码（买服务器时设置的 root 密码）。",

    nameLabel: "项目名称",
    namePlaceholder: "my-project",
    nameHint: "随便起，之后可以改。",
    needName: "给项目起个名字，例如 my-project。",
    remoteLabel: "服务器上的项目目录",
    remoteHint: "目录不存在会自动创建，你不需要懂 Linux 路径。",
    remoteMustBeAbsolute: "远端目录必须以 / 开头。",
    localLabel: "电脑上的同步文件夹",
    localHint: "服务器上的文件会实时同步到这里，用你熟悉的软件随时打开。",
    autoSet: "已自动设置",
    change: "修改",
    useRecommended: "用推荐值",

    excludesSummary: "高级选项：同步排除规则",
    excludesLabel: "不同步的内容（每行一条，已按最佳实践预设）",
    protectSecrets: "🛡 机密文件（.env、密钥）默认受保护，永不同步。",

    summaryProject: "项目",
    summaryServer: "服务器",
    summaryVerified: "✓ 已验证连接",
    summaryRemote: "服务器目录",
    summaryLocal: "本地文件夹",
    summaryHint: "点「完成设置」后一切自动进行，大约需要 1 分钟。",
    finish: "完成设置",
    deploying: "设置中…",
    saveChanges: "保存修改",
    saved: "项目设置已保存。",

    stageConnect: "连接服务器",
    stageInstall: "安装CC避风港同步组件（自动完成，无需操作）",
    stageDirectory: "创建项目目录 {path}",
    stageSync: "首次同步并启动 Claude Code 终端",
    stageSyncProgress: "首次同步（{detail}）",
  },

  workspace: {
    openLocalFolder: "打开本地文件夹",
    connected: "已连接 · {sync}",
    disconnected: "连接已断开",
    reconnect: "重新连接",
    tabTerminal: "终端",
    tabFiles: "文件",
    tabConflicts: "冲突",

    terminalConnecting: "正在连接 {host}…",
    terminalDropped: "连接已断开，{n} 秒后自动重连…",
    terminalReconnectNow: "立即重连",
    terminalReconnected: "已恢复连接，继续同步…",
    terminalFailedTitle: "终端启动失败",
    terminalDiagnostics: "查看诊断",

    explorer: "资源管理器",
    newFile: "新建文件",
    newFolder: "新建文件夹",
    collapseAll: "全部折叠",
    open: "打开",
    rename: "重命名",
    filesEmpty: "尚未同步任何文件",
    filesEmptyHint: "服务器上的文件同步过来后会出现在这里。",
    filesError: "无法读取本地同步文件夹。",
    fileNamePlaceholder: "文件名（含扩展名）",
    folderNamePlaceholder: "文件夹名称",
    createdSyncing: "已创建 {name}，正在同步到服务器…",
    renamedSyncing: "已重命名为 {name}，正在同步到服务器…",
    deletedBothSides: "已删除 {name}（两端同步删除）。",
    deleteUndone: "已撤销删除 {name}。",
    refreshed: "已刷新，与服务器一致。",

    recentTitle: "最近更新",
    recentBody: "服务器上改了什么，这里一目了然。点击文件查看内容。",
    tooLarge: "文件过大，无法预览。",
    binaryFile: "二进制文件，无法预览。",
    goToConflicts: "去「冲突」页处理 →",
    modifiedAt: "修改于 {when}",

    conflictsEmptyTitle: "没有冲突，已全部同步 ✓",
    conflictsEmptyBody: "当两边同时修改同一文件时，会在这里显示供你判断。",
    keepLocal: "保留本地",
    keepRemote: "保留远端",
    keepBoth: "两者都保留（另存副本）",
    resolveAll: "全部按…解决",
    localPane: "本地 — {when}修改",
    remotePane: "远端 — {when}修改",
    remoteDeleted: "远端已删除该文件",
    resolved: "已解决 {path} — {how}。",
    resolveUndone: "已撤销，{path} 重新回到冲突列表。",
  },

  account: {
    subscribed: "已订阅 · 有效期至 {date}（剩余 {n} 天）",
    trialing: "免费试用中 · 剩余 {n} 天",
    none: "未订阅",
    chipSubscribed: "已订阅 · 剩 {n} 天",
    chipTrialing: "免费试用 · 剩 {n} 天",
    chipNone: "未订阅",
    manage: "管理订阅与账号 ↗",
    invite: "邀请好友 ↗",
    docs: "使用文档 ↗",
    support: "联系我们 ↗",
    logout: "退出登录",
    expiringBanner: "订阅即将到期，去官网续费 ↗",
  },

  sync: {
    label: {
      synced: "已同步",
      syncing: "正在同步",
      conflicts: "有冲突",
      offline: "离线",
    },
  },
} as const;

export type Dictionary = typeof zhCN;
