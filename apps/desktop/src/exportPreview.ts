export const exportPreviewMaxCharacters = 64 * 1024;
export const exportPreviewMaxLines = 500;

export interface ExportPreviewLimits {
  maxCharacters?: number;
  maxLines?: number;
}

const normalizedLimit = (value: number | undefined, fallback: number): number => {
  if (!Number.isFinite(value)) return fallback;
  return Math.max(1, Math.floor(value as number));
};

export const buildExportPreview = (
  content: string,
  notice: string,
  limits: ExportPreviewLimits = {},
): string => {
  const maxCharacters = normalizedLimit(limits.maxCharacters, exportPreviewMaxCharacters);
  const maxLines = normalizedLimit(limits.maxLines, exportPreviewMaxLines);
  let end = Math.min(content.length, maxCharacters);
  let lineCount = 1;

  for (let index = 0; index < end; index += 1) {
    if (content.charCodeAt(index) !== 10) continue;
    lineCount += 1;
    if (lineCount > maxLines) {
      end = index;
      break;
    }
  }

  if (end === content.length) return content;
  if (
    end > 0 &&
    end < content.length &&
    content.charCodeAt(end - 1) >= 0xd800 &&
    content.charCodeAt(end - 1) <= 0xdbff &&
    content.charCodeAt(end) >= 0xdc00 &&
    content.charCodeAt(end) <= 0xdfff
  ) {
    end -= 1;
  }

  const prefix = content.slice(0, end).replace(/\s+$/u, "");
  const label = notice.trim() || "Preview truncated.";
  return prefix ? `${prefix}\n\n${label}` : label;
};
