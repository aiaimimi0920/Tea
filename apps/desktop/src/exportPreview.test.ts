import { describe, expect, it } from "vitest";

import { buildExportPreview } from "./exportPreview";

describe("export preview bounds", () => {
  it("keeps small exports unchanged", () => {
    expect(buildExportPreview("small export", "truncated")).toBe("small export");
  });

  it("caps characters and keeps the full-export notice", () => {
    expect(buildExportPreview("abcdef", "Download the full export.", { maxCharacters: 4 })).toBe(
      "abcd\n\nDownload the full export.",
    );
  });

  it("caps lines before rendering a large timeline", () => {
    expect(buildExportPreview("one\ntwo\nthree", "truncated", { maxLines: 2 })).toBe(
      "one\ntwo\n\ntruncated",
    );
  });

  it("does not split a Unicode surrogate pair", () => {
    expect(buildExportPreview("abcd😀z", "truncated", { maxCharacters: 5 })).toBe(
      "abcd\n\ntruncated",
    );
  });
});
