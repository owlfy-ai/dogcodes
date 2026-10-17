import type { CSSProperties } from "react";

const lines = [[64, 0], [42, 0], [76, 1], [55, 2], [68, 2], [34, 1], [58, 0], [72, 1], [46, 1], [30, 0]];

export function ReviewFileLoading({ split = false }: { split?: boolean }) {
  return <div className="review-file-loading" role="status" aria-label="正在加载文件内容">
    <div className="review-loading-columns" data-split={split} aria-hidden="true">
      {Array.from({ length: split ? 2 : 1 }, (_, column) => <div className="review-loading-column" key={column}>
        {lines.map(([width, indent], index) => <div className="review-loading-line" key={index}
          style={{ "--loading-line-width": `${width}%`, "--loading-line-indent": `${indent}em` } as CSSProperties}>
          <span className="review-loading-gutter" /><span className="review-loading-code" />
        </div>)}
      </div>)}
    </div>
  </div>;
}
