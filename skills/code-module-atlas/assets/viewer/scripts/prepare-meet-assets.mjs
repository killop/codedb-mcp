import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(here);
const htmlPath = join(root, "index.html");
const assetPath = join(root, "public", "assets", "index-DB71QehS.js");
const onlineKey = 'const Ss="514ec6a7c0b25f7d22fd4ecf551d5e186dea5aaf3cb7c20ace5f4afb08f9dbef";';
const localKey = 'const Ss="";';

function replaceBetween(source, start, end, replacement) {
  const startIndex = source.indexOf(start);
  if (startIndex < 0) {
    return source;
  }
  const endIndex = source.indexOf(end, startIndex);
  if (endIndex < 0) {
    return source;
  }
  return source.slice(0, startIndex) + replacement + source.slice(endIndex);
}

function patchAsset(source) {
  source = source.replace(
    "function ps(o,t){return t?wi(o)===t||Mn(o).includes(t):!0}",
    "function ps(o,t){return t?typeof t===\"string\"&&t.indexOf(\"module:\")===0?String(o.moduleId)===t.slice(7):wi(o)===t||Mn(o).includes(t):!0}"
  );
  source = source.replace(
    "S=Ya(o.nodes,_=>{w(_)});async function w(_)",
    "S=Ya(o.nodes,_=>{w(_)},f);async function w(_)"
  );
  source = source.replace("const T=_||\"全部\";", "const T=S.label?S.label(_):(_||\"全部\");");
  source = source.replace("x.setNodes(y.nodes,_?`仅 ${_}`:\"\")", "x.setNodes(y.nodes,_?`仅 ${T}`:\"\")");
  source = source.replace(
    "ar(_?`${_} 子星图 · ${y.nodes.length} 节点 · ${y.edges.length} 关系`:`全部文件 · ${y.nodes.length} 节点`)",
    "ar(_?`${T} · ${y.nodes.length} 文件 · ${y.edges.length} 依赖`:`全部文件 · ${y.nodes.length} 节点`)"
  );
  source = source.replace(
    'onDblClick(_){pe("open",{url:_.url,title:_.title,depth:_.depth}),window.open(_.url,"_blank","noopener,noreferrer")}',
    'onDblClick(_){pe("dblclick",{url:_.url,title:_.title,depth:_.depth})}'
  );
  const detailStart = source.includes("function atlasPanelEscape(") ? "function atlasPanelEscape(" : "function lr(o,t,e){";
  const detailPanelPatch = String.raw`function atlasPanelEscape(o){return String(o??"").replace(/[&<>"']/g,t=>({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[t]))}function atlasPanelValue(o){return o===void 0||o===null||o===""?"-":String(o)}function atlasPanelRefs(o){const t=(o??[]).slice(0,6);if(t.length===0)return'<div class="atlas-ref-empty">暂无</div>';const e=t.map(i=>'<div class="atlas-ref-item" title="'+atlasPanelEscape(i.path||i.title||"")+'"><span>'+atlasPanelEscape(i.title||i.path||"未知文件")+'</span><small>'+atlasPanelEscape(i.path||"")+'</small></div>').join("");const n=(o??[]).length>t.length?'<div class="atlas-ref-more">另有 '+((o??[]).length-t.length)+' 个</div>':"";return'<div class="atlas-ref-list">'+e+n+"</div>"}function atlasPanelDrag(){if(!oi||oi.dataset.dragReady==="1")return;oi.dataset.dragReady="1";const o=oi.querySelector(".panel-icon-wrap")||oi;o.classList.add("atlas-panel-drag-handle"),o.addEventListener("pointerdown",t=>{if(t.button!==void 0&&t.button!==0)return;if(t.target&&t.target.closest("button"))return;oi.dataset.dragged="1",oi.classList.add("atlas-dragging");const e=oi.getBoundingClientRect(),i=t.clientX-e.left,n=t.clientY-e.top;function r(s){const a=12,l=oi.offsetWidth||320,c=oi.offsetHeight||260,u=Math.max(a,Math.min(window.innerWidth-l-a,s.clientX-i)),d=Math.max(a,Math.min(window.innerHeight-c-a,s.clientY-n));oi.style.left=String(u)+"px",oi.style.top=String(d)+"px"}function s(){oi.classList.remove("atlas-dragging"),window.removeEventListener("pointermove",r)}o.setPointerCapture&&o.setPointerCapture(t.pointerId),window.addEventListener("pointermove",r),window.addEventListener("pointerup",s,{once:!0}),t.preventDefault()})}function lr(o,t,e){atlasPanelDrag(),La.textContent=o.title||"未知文件";const i=o.path||o.url||"";if(nr.textContent=i,nr.title=i,nr.removeAttribute("href"),nr.removeAttribute("target"),nr.removeAttribute("rel"),sr){const d=wi(o),f=Mn(o);sr.innerHTML='<span class="panel-cat-primary">'+atlasPanelEscape(d)+"</span>"+f.map(p=>"<span>"+atlasPanelEscape(p)+"</span>").join("")}const n=Array.isArray(o.symbols)?o.symbols:[],r=n.slice(0,12).map(d=>'<span class="atlas-symbol-pill">'+atlasPanelEscape(d)+"</span>").join(""),s=n.length>12?'<span class="atlas-symbol-more">+'+(n.length-12)+"</span>":"",a=atlasPanelEscape(o.moduleLabel??o.moduleId??"-"),l=atlasPanelEscape(o.languageLabel??o.language??"-"),c=atlasPanelEscape(atlasPanelValue(o.lineCount)),u=atlasPanelEscape(atlasPanelValue(o.symbolCount??n.length));Da.innerHTML='<div class="atlas-detail-body"><div class="atlas-detail-row"><span class="atlas-detail-label">路径</span><span class="atlas-detail-value atlas-detail-path" title="'+atlasPanelEscape(i)+'">'+atlasPanelEscape(i)+'</span></div><div class="atlas-detail-row"><span class="atlas-detail-label">模块</span><span class="atlas-detail-value">'+a+'</span></div><div class="atlas-detail-metrics"><div><span>语言</span><strong>'+l+'</strong></div><div><span>行数</span><strong>'+c+'</strong></div><div><span>符号</span><strong>'+u+'</strong></div></div><div class="atlas-detail-section"><div class="atlas-detail-section-title">主要符号</div><div class="atlas-symbol-list">'+(r||'<span class="atlas-ref-empty">暂无</span>')+s+'</div></div><div class="atlas-detail-section"><div class="atlas-detail-section-title">被这些文件引用</div>'+atlasPanelRefs(o.incomingRefs)+'</div><div class="atlas-detail-section"><div class="atlas-detail-section-title">引用这些文件</div>'+atlasPanelRefs(o.outgoingRefs)+"</div></div>",Ia.textContent=String(o.inDegree??0),za.textContent=String(o.outDegree??0),Na.textContent=String((o.inDegree??0)+(o.outDegree??0)),Di.src=o.iconUrl||"",Di.onerror=()=>{Di.src="https://www.google.com/s2/favicons?sz=64&domain_url="+encodeURIComponent(o.url||"")};if(oi.dataset.dragged!=="1"){const d=340,f=360,p=16,m=24;let h=t+m;h+d>window.innerWidth-p&&(h=t-d-m),h=Math.max(p,Math.min(window.innerWidth-d-p,h));let g=e-f/2;g=Math.max(p,Math.min(window.innerHeight-f-p,g)),oi.style.left=String(h)+"px",oi.style.top=String(g)+"px"}oi.classList.add("open"),ys(o.id)}`;
  source = replaceBetween(source, detailStart, "function En(){", detailPanelPatch);
  const moduleNavStart = source.includes("function Ya(o,t,e){") ? "function Ya(o,t,e){" : "function Ya(o,t){";
  source = replaceBetween(source, moduleNavStart, "function Wa(", String.raw`function Ya(o,t,e){if(!$e||!rr||!gi)return{setActive(){},setBusy(){},label(i){return i||"全部文件"}};const i=$e,n=rr,r=gi,s=new Map;for(const h of o){const g=h.moduleId??"unknown",b=h.moduleLabel??String(g);let x=s.get(String(g));x||(x={id:String(g),label:b,files:[],count:0},s.set(String(g),x)),x.files.push(h),x.count++}const a=[...s.values()].sort((h,g)=>g.count-h.count||h.label.localeCompare(g.label));let l="",c=null;function u(h){return String(h??"").replace(/[&<>"']/g,g=>({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[g]))}function d(h){return h&&h.indexOf("module:")===0?h.slice(7):""}function f(h){if(!h)return"全部文件";const g=s.get(d(h));return g?g.label:h}function p(){const R=n.querySelector(".atlas-module-list"),D=R?R.scrollTop:0;n.innerHTML="";const h=document.createElement("div");h.className="atlas-nav";const g=document.createElement("div");g.className="atlas-panel-section atlas-module-section";g.innerHTML='<div class="atlas-section-title"><span>模块列表</span><span>'+a.length+"</span></div>";const b=document.createElement("div");b.className="atlas-list atlas-module-list";const x=document.createElement("button");x.className="atlas-list-item atlas-module-item"+(l===""?" active":"");x.innerHTML='<span class="atlas-main">全部文件</span><span class="atlas-count">'+o.length+"</span>";x.addEventListener("click",()=>{l="",p(),m(null),t("")}),b.appendChild(x);for(const S of a){const w="module:"+S.id,v=document.createElement("button");v.className="atlas-list-item atlas-module-item"+(l===w?" active":"");v.title=S.label;v.innerHTML='<span class="atlas-main">'+u(S.label)+'</span><span class="atlas-count">'+S.count+"</span>";v.addEventListener("click",()=>{if(l===w)return;l=w,p(),m(S.id),t(w)}),b.appendChild(v)}g.appendChild(b);const T=document.createElement("div");T.className="atlas-panel-section atlas-file-section";T.innerHTML='<div class="atlas-section-title"><span>文件列表</span><span id="atlas-file-count">0</span></div><div class="atlas-list atlas-file-list" id="atlas-file-list"></div>';h.appendChild(g),h.appendChild(T),n.appendChild(h),b.scrollTop=D}function m(h){const g=document.getElementById("atlas-file-list"),b=document.getElementById("atlas-file-count");if(!g||!b)return;g.innerHTML="",c=null;if(!h){b.textContent="";const E=document.createElement("div");E.className="atlas-empty";E.textContent="选择一个模块后显示文件";g.appendChild(E),r.textContent="全部文件 · "+o.length+" 节点";return}const x=s.get(String(h));if(!x){b.textContent="0";return}const S=[...x.files].sort((E,A)=>(A.inDegree+A.outDegree)-(E.inDegree+E.outDegree)||E.title.localeCompare(A.title));b.textContent=String(S.length),r.textContent=x.label+" · "+S.length+" 文件";for(const E of S){const A=document.createElement("button");A.className="atlas-list-item atlas-file-item";A.title=E.path||E.url;A.innerHTML='<span class="atlas-main">'+u(E.title||E.url)+'</span><span class="atlas-sub">'+u(E.path||E.url)+'</span>';A.addEventListener("click",()=>{c=E.id,g.querySelectorAll(".atlas-file-item").forEach(O=>O.classList.remove("active")),A.classList.add("active"),e&&e(E.id)}),g.appendChild(A)}}function h(g){l=g||"",p(),m(d(l))}return p(),m(null),{setActive:h,setBusy(g){i.classList.toggle("busy",g),n.querySelectorAll("button").forEach(b=>{b.disabled=g})},label:f}}`);
  return source;
}

function patchHtml(source) {
  let patched = source;
  const css = String.raw`

      /* atlas-module-sidebar */
      #category-dock {
        width: 318px;
        bottom: 24px;
      }
      #category-chips {
        display: block;
        max-height: calc(100vh - 110px);
        overflow: hidden;
        padding-right: 0;
      }
      .atlas-nav {
        display: grid;
        gap: 10px;
      }
      .atlas-panel-section {
        background: rgba(4, 12, 35, 0.72);
        border: 1px solid rgba(100,200,255,0.16);
        border-radius: 12px;
        backdrop-filter: blur(12px);
        overflow: hidden;
      }
      .atlas-section-title {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 8px 10px;
        color: #4d8ba8;
        font-size: 0.72rem;
        letter-spacing: 0.06em;
        border-bottom: 1px solid rgba(100,200,255,0.08);
      }
      .atlas-list {
        overflow: auto;
      }
      .atlas-module-list {
        max-height: 38vh;
      }
      .atlas-file-list {
        max-height: 34vh;
      }
      .atlas-list::-webkit-scrollbar {
        width: 4px;
      }
      .atlas-list::-webkit-scrollbar-thumb {
        background: rgba(100,200,255,0.22);
        border-radius: 2px;
      }
      .atlas-list-item {
        width: 100%;
        display: grid;
        grid-template-columns: minmax(0, 1fr) auto;
        gap: 8px;
        align-items: center;
        padding: 7px 10px;
        border: 0;
        border-bottom: 1px solid rgba(100,200,255,0.06);
        background: transparent;
        color: #4d8ba8;
        text-align: left;
        cursor: pointer;
      }
      .atlas-list-item:hover,
      .atlas-list-item.active {
        color: #8ee8ff;
        background: rgba(0,160,255,0.14);
      }
      .atlas-main {
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        font-size: 0.75rem;
      }
      .atlas-sub {
        grid-column: 1 / -1;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        color: #2f6d8c;
        font-size: 0.66rem;
      }
      .atlas-count {
        color: #2f6d8c;
        font-size: 0.68rem;
        font-variant-numeric: tabular-nums;
      }
      .atlas-empty {
        padding: 14px 10px;
        color: #2f6d8c;
        font-size: 0.72rem;
      }
      @media (max-width: 760px) {
        #category-dock {
          top: 64px;
          width: auto;
        }
        #category-chips {
          max-height: 45vh;
        }
        .atlas-module-list,
        .atlas-file-list {
          max-height: 20vh;
        }
      }
`;
  if (!patched.includes("atlas-module-sidebar")) {
    patched = patched.replace(/\n\s*<\/style>/, `${css}\n    </style>`);
  }
  const detailCss = String.raw`

      /* atlas-detail-panel */
      #info-panel {
        width: min(380px, calc(100vw - 32px));
        max-height: calc(100vh - 32px);
        overflow: hidden auto;
      }
      #info-panel.atlas-dragging {
        transition: none;
      }
      .atlas-panel-drag-handle {
        cursor: move;
      }
      #panel-url {
        cursor: default;
      }
      #panel-url:hover {
        color: #3399bb;
      }
      .atlas-detail-body {
        display: grid;
        gap: 10px;
      }
      .atlas-detail-row {
        display: grid;
        grid-template-columns: 52px minmax(0, 1fr);
        gap: 10px;
        align-items: start;
        padding: 8px 10px;
        background: rgba(0, 100, 180, 0.08);
        border: 1px solid rgba(100, 200, 255, 0.08);
        border-radius: 8px;
      }
      .atlas-detail-label,
      .atlas-detail-section-title,
      .atlas-detail-metrics span {
        color: #336688;
        font-size: 0.68rem;
        letter-spacing: 0.04em;
      }
      .atlas-detail-value {
        min-width: 0;
        color: #a9dff2;
        font-size: 0.75rem;
        line-height: 1.45;
        word-break: break-word;
      }
      .atlas-detail-path {
        font-family: Consolas, "Cascadia Mono", monospace;
      }
      .atlas-detail-metrics {
        display: grid;
        grid-template-columns: repeat(3, minmax(0, 1fr));
        gap: 8px;
      }
      .atlas-detail-metrics div {
        display: grid;
        gap: 3px;
        padding: 8px 10px;
        background: rgba(0, 100, 180, 0.1);
        border: 1px solid rgba(100, 200, 255, 0.08);
        border-radius: 8px;
      }
      .atlas-detail-metrics strong {
        overflow: hidden;
        color: #7df8ff;
        font-size: 0.78rem;
        font-weight: 600;
        text-overflow: ellipsis;
        white-space: nowrap;
      }
      .atlas-detail-section {
        display: grid;
        gap: 7px;
      }
      .atlas-symbol-list {
        display: flex;
        flex-wrap: wrap;
        gap: 6px;
      }
      .atlas-symbol-pill,
      .atlas-symbol-more {
        max-width: 100%;
        overflow: hidden;
        padding: 3px 7px;
        border: 1px solid rgba(100, 200, 255, 0.12);
        border-radius: 999px;
        background: rgba(0, 200, 255, 0.06);
        color: #6dc5de;
        font-size: 0.68rem;
        text-overflow: ellipsis;
        white-space: nowrap;
      }
      .atlas-ref-list {
        display: grid;
        gap: 5px;
      }
      .atlas-ref-item {
        min-width: 0;
        padding: 7px 9px;
        border-left: 2px solid rgba(125, 248, 255, 0.28);
        background: rgba(0, 100, 180, 0.08);
        border-radius: 6px;
      }
      .atlas-ref-item span,
      .atlas-ref-item small {
        display: block;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
      }
      .atlas-ref-item span {
        color: #a9dff2;
        font-size: 0.72rem;
      }
      .atlas-ref-item small,
      .atlas-ref-empty,
      .atlas-ref-more {
        color: #336688;
        font-size: 0.66rem;
      }
`;
  if (!patched.includes("atlas-detail-panel")) {
    patched = patched.replace(/\n\s*<\/style>/, `${detailCss}\n    </style>`);
  }
  return patched;
}

function rewrite(path) {
  let source = readFileSync(path, "utf8");
  source = source.replace(onlineKey, localKey);
  source = source.replaceAll("        attribute vec3 instanceColor;\n", "");
  source = source.replace(/function On\(o\)\{const t=o\.inDegree\+o\.outDegree;return [^}]+}/, "function On(o){const t=o.inDegree+o.outDegree;return .35+Math.pow(t/Math.max(1,mr),.42)*2.6}");
  source = source.replaceAll("new zs(1,12,8)", "new zs(.62,10,6)");
  source = source.replaceAll("n[a]=On(l)*3.5", "n[a]=On(l)*2.1");
  source = source.replaceAll("n[a]=On(l)*2.1", "n[a]=On(l)*1.45");
  source = source.replaceAll("gl_PointSize = clamp(aSz * (320.0 / -mv.z), 1.5, 48.0);", "gl_PointSize = clamp(aSz * (260.0 / -mv.z), 1.0, 18.0);");
  source = source.replaceAll("gl_PointSize = clamp(aSz * (260.0 / -mv.z), 1.0, 18.0);", "gl_PointSize = clamp(aSz * (240.0 / -mv.z), 1.0, 10.0);");
  source = source.replaceAll("gl_FragColor = vec4(vCol * 1.4, a * 0.75);", "gl_FragColor = vec4(vCol * 1.15, a * 0.45);");
  source = source.replaceAll("const z=.38", "const z=.24");
  source = source.replaceAll("opacity:.55", "opacity:.32");
  source = source.replaceAll("t===this.selectedIdx?n=i*1.6:this.pathNodeSet.has(t)?n=i*1.45:this.networkNodeSet.has(t)?n=i*1.35:t===this.hoveredIdx&&(n=i*1.3)", "t===this.selectedIdx?n=i*1.25:this.pathNodeSet.has(t)?n=i*1.18:this.networkNodeSet.has(t)?n=i*1.15:t===this.hoveredIdx&&(n=i*1.12)");
  source = source.replace(/ol\(\)\.then\(\(\)=>ws\(\)\)\.catch\(o=>console\.warn\("user-ui 初始化失败:",o\)\);/, "/* user-ui disabled for local codedb viewer */");
  source = source.replaceAll("MeetBlog · 中文博客星系", "codedb File Atlas");
  source = source.replaceAll("codedb Module Atlas", "codedb File Atlas");
  source = source.replaceAll("MEET BLOG", "CODEDB ATLAS");
  source = source.replaceAll("中文独立博客星系", "代码文件星系");
  source = source.replaceAll("中文独立博客", "代码文件");
  source = source.replaceAll("博客圈", "代码库");
  source = source.replaceAll("博客导航", "文件导航");
  source = source.replaceAll("博客索引", "文件索引");
  source = source.replaceAll("博客星系", "文件星系");
  source = source.replaceAll("博客网络", "文件依赖网络");
  source = source.replaceAll("博客间", "文件间");
  source = source.replaceAll("博客标题", "文件名");
  source = source.replaceAll("未知博客", "未知文件");
  source = source.replaceAll("访问博客", "查看文件");
  source = source.replaceAll("测试博客", "测试文件");
  source = source.replaceAll("博客", "文件");
  source = source.replaceAll("代码模块", "代码文件");
  source = source.replaceAll("模块名称", "文件名");
  source = source.replaceAll("模块节点", "文件节点");
  source = source.replaceAll("模块描述", "文件描述");
  source = source.replaceAll("模块依赖", "文件依赖");
  source = source.replaceAll("模块间", "文件间");
  source = source.replaceAll("全部模块", "全部文件");
  source = source.replaceAll("个模块", "个文件");
  source = source.replaceAll("查看模块", "查看文件");
  source = source.replaceAll("未知模块", "未知文件");
  source = source.replaceAll("测试模块", "测试文件");
  source = source.replaceAll("友链", "依赖");
  source = source.replaceAll("爬虫", "索引器");
  source = source.replaceAll("爬取", "索引");
  source = source.replaceAll("网站", "文件");
  source = source.replaceAll("双击查看文件", "点击选择节点");
  if (path === assetPath) {
    source = patchAsset(source);
  }
  if (path === htmlPath) {
    source = patchHtml(source);
  }
  writeFileSync(path, source, "utf8");
}

rewrite(htmlPath);
rewrite(assetPath);
console.log("prepared meet-blog frontend for local codedb dataset");
