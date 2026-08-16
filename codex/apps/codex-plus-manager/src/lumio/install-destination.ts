export interface DestinationOption {
  readonly id: "standard" | "choose";
  readonly label: string;
  readonly note: string | null;
}

/**
 * 首次安装的「安装位置」步骤（D-23）：Windows 的标准路线是 MSIX，装哪由系统管，
 * 选目录只能兑现到便携解压——两条都摆出来，不默认替用户做取舍。
 */
export function destinationOptions(platform: string): readonly DestinationOption[] {
  if (platform === "macos") {
    return [
      { id: "standard", label: "默认位置（/Applications）", note: null },
      { id: "choose", label: "选择文件夹…", note: null },
    ];
  }
  return [
    {
      id: "standard",
      label: "标准安装（推荐）",
      note: "安装到 Windows 管理的位置；之后可在 系统设置 → 应用 中「移动」到其他盘",
    },
    { id: "choose", label: "选择安装目录…", note: "解压安装到所选目录，直接运行" },
  ];
}
