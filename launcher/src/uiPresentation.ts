export const deleteConfirmationText = (label: string) =>
  `确定删除「${label}」？删除后无法恢复。`;

export const screenMessage = (value: string) => {
  const withoutLeadingCode = value.trim().replace(
    /^[a-z][a-z0-9_-]*(?:\.[a-z0-9_-]+)+(?:\s*:\s*|$)/i,
    "",
  );
  return withoutLeadingCode || "查看完整诊断";
};
