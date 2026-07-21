const copyButton = document.querySelector("[data-copy]");
const copyStatus = document.querySelector("#copy-status");
const previewButton = document.querySelector("[data-replay-src]");
const previewImage = document.querySelector("[data-replay-preview]");
const previewStatus = document.querySelector("[data-preview-status]");

function setPreviewControl({ playing, busy = false }) {
  if (!previewButton) return;
  const icon = previewButton.querySelector(".preview-control-icon");
  const title = previewButton.querySelector(".preview-control-copy strong");
  const detail = previewButton.querySelector(".preview-control-copy small");
  previewButton.disabled = busy;
  previewButton.setAttribute("aria-pressed", String(playing));
  if (icon) icon.textContent = busy ? "…" : playing ? "■" : "▶";
  if (title) title.textContent = busy ? "Loading preview" : playing ? "Stop preview" : "Play 15-second preview";
  if (detail) detail.textContent = playing ? "return to the still frame" : "2.1 MB · recorded replay";
}

previewButton?.addEventListener("click", () => {
  if (!(previewImage instanceof HTMLImageElement)) return;
  const animationSrc = previewButton.getAttribute("data-replay-src");
  const posterSrc = previewButton.getAttribute("data-poster-src");
  const isPlaying = previewButton.getAttribute("aria-pressed") === "true";
  if (!animationSrc || !posterSrc) return;

  if (isPlaying) {
    previewImage.src = posterSrc;
    setPreviewControl({ playing: false });
    if (previewStatus) previewStatus.textContent = "Recorded preview stopped.";
    return;
  }

  setPreviewControl({ playing: false, busy: true });
  if (previewStatus) previewStatus.textContent = "Loading the recorded preview.";
  const animation = new Image();
  animation.onload = () => {
    previewImage.src = animation.src;
    setPreviewControl({ playing: true });
    if (previewStatus) previewStatus.textContent = "Recorded preview playing.";
  };
  animation.onerror = () => {
    setPreviewControl({ playing: false });
    if (previewStatus) {
      previewStatus.textContent = "Preview could not load. Open the full recorded replay instead.";
    }
  };
  animation.src = animationSrc;
});

copyButton?.addEventListener("click", async () => {
  const selector = copyButton.getAttribute("data-copy");
  const source = selector ? document.querySelector(selector) : null;
  const text = source?.textContent?.trim();
  if (!text) return;

  try {
    await navigator.clipboard.writeText(text);
    copyButton.textContent = "Copied";
    if (copyStatus) copyStatus.textContent = "Build commands copied to the clipboard.";
    window.setTimeout(() => {
      copyButton.textContent = "Copy";
    }, 2_000);
  } catch {
    if (copyStatus) {
      copyStatus.textContent = "Copy failed. Select the commands in the terminal block instead.";
    }
  }
});
