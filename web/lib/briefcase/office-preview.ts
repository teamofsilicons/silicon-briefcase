import { strFromU8, unzipSync } from "fflate";
const MAX_XML = 12 * 1024 * 1024;
const escape = (value: string) => value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");
const elements = (doc: Document | Element, name: string) => Array.from(doc.getElementsByTagNameNS("*", name));
const xml = (data: Uint8Array | undefined) => {
  if (!data) throw new Error("The document does not contain a readable preview.");
  const doc = new DOMParser().parseFromString(strFromU8(data), "application/xml");
  if (doc.getElementsByTagName("parsererror").length) throw new Error("The document’s XML could not be read.");
  return doc;
};
/** Text and tables only. External links, scripts, macros and embedded objects are never executed. */
export function officePreview(data: Uint8Array, extension: string): string {
  let expanded = 0;
  const files = unzipSync(data, { filter: entry => {
    if (!/^(word\/document\.xml|xl\/(sharedStrings\.xml|worksheets\/sheet\d+\.xml)|ppt\/slides\/slide\d+\.xml|content\.xml)$/.test(entry.name)) return false;
    expanded += entry.originalSize;
    if (expanded > MAX_XML) throw new Error("The document is too large to preview. Download it to open the complete file.");
    return true;
  } });
  if (extension === "docx") return elements(xml(files["word/document.xml"]), "p").slice(0,1000).map(p => `<p>${escape(elements(p,"t").map(t => t.textContent || "").join(""))}</p>`).join("");
  if (extension === "xlsx" || extension === "xlsm") {
    const shared = files["xl/sharedStrings.xml"] ? elements(xml(files["xl/sharedStrings.xml"]),"si").map(si => elements(si,"t").map(t => t.textContent || "").join("")) : [];
    return Object.keys(files).filter(name => name.startsWith("xl/worksheets/")).sort().slice(0,20).map((name,index) => `<h2>Sheet ${index + 1}</h2><table>${elements(xml(files[name]),"row").slice(0,300).map(row => `<tr>${elements(row,"c").slice(0,50).map(cell => { const raw = elements(cell,"v")[0]?.textContent || elements(cell,"t")[0]?.textContent || ""; const value = cell.getAttribute("t") === "s" ? shared[Number(raw)] || "" : raw; return `<td>${escape(value)}</td>`; }).join("")}</tr>`).join("")}</table>`).join("");
  }
  if (extension === "pptx") return Object.keys(files).filter(name => name.startsWith("ppt/slides/")).sort((a,b) => Number(a.match(/slide(\d+)/)?.[1]) - Number(b.match(/slide(\d+)/)?.[1])).slice(0,100).map((name,index) => `<section><h2>Slide ${index + 1}</h2>${elements(xml(files[name]),"p").map(p => `<p>${escape(elements(p,"t").map(t => t.textContent || "").join(""))}</p>`).join("")}</section>`).join("");
  const doc = xml(files["content.xml"]);
  if (extension === "ods") return elements(doc,"table").slice(0,20).map(table => `<table>${elements(table,"table-row").slice(0,300).map(row => `<tr>${elements(row,"table-cell").slice(0,50).map(cell => `<td>${escape(cell.textContent || "")}</td>`).join("")}</tr>`).join("")}</table>`).join("");
  return elements(doc,"p").slice(0,1000).map(p => `<p>${escape(p.textContent || "")}</p>`).join("");
}
export function sandboxDocument(body: string): string {
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'"><style>body{font:15px/1.7 system-ui;color:#292929;background:#fff;margin:28px}p{min-height:1em}table{border-collapse:collapse;margin:20px 0;max-width:100%}td{border:1px solid #d9dde4;padding:8px 12px;white-space:pre-wrap}section{border-bottom:1px solid #d9dde4;padding:10px 0 24px}h2{font-size:18px}</style></head><body>${body}</body></html>`;
}
