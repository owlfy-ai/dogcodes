import { useState } from "react";

function ImageVersion({ src, label, showLabel }: { src: string; label: string; showLabel: boolean }) {
  const [failed, setFailed] = useState(false);
  return <figure>{showLabel && <figcaption>{label}</figcaption>}{failed ? <p>图片无法加载</p> : <img src={src} alt={label} onError={() => setFailed(true)} />}</figure>;
}
export function ReviewBinaryPlaceholder({ beforeImage, afterImage }: { beforeImage?: string | null; afterImage?: string | null }) {
  if (beforeImage || afterImage) return <div className="review-image-preview">
    {beforeImage && <ImageVersion key={beforeImage} src={beforeImage} label="修改前" showLabel={Boolean(afterImage)} />}
    {afterImage && <ImageVersion key={afterImage} src={afterImage} label="修改后" showLabel={Boolean(beforeImage)} />}
  </div>;
  return <div className="review-binary-placeholder" role="status">
    <svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.3"><path d="M6 3h8l4 4v14H6zM14 3v5h4M9 12h6M9 16h4" /></svg>
    <p>此二进制文件暂不支持预览</p>
  </div>;
}
