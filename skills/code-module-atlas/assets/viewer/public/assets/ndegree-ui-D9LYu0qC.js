import{o as B,g as H,a as z}from"./index-DB71QehS.js";import{b as R,a as P}from"./pathfinder-wgwU7SjB.js";import"./three-Df2n1lyU.js";import"./postprocessing-D4VCwszR.js";const $="meetblog:tool-open";let e=null,t=null,k=null,L=!1;function K(r){return e={graph:null,rows:[],scopeLabel:"当前星图",selected:null,degree:1,busy:!1,callbacks:r,lastResult:null},C(),k||(k=B(n=>{n||h()})),{setView(n,o,d){e&&(e.graph=R(n,o),e.rows=U(n),e.scopeLabel=d,e.selected&&!e.graph.nodeById.has(e.selected.id)&&(e.selected=null),p(!1),E(),m(),u())},clearNetwork(){p()}}}function Q(){C(),H()&&(window.dispatchEvent(new CustomEvent($,{detail:"ndegree-network"})),t.classList.add("open"),E(),m(),l(e!=null&&e.graph?"":"星图仍在加载，稍后即可使用 N 度网络。","muted"),u(),setTimeout(()=>{var r;return(r=t.querySelector("#nd-node-input"))==null?void 0:r.focus()},40))}function h(){t==null||t.classList.remove("open"),b()}function C(){return W(),j(),t||(t=document.createElement("div"),t.id="ndegree-panel",t.innerHTML=`
    <div class="nd-head">
      <div>
        <div class="nd-kicker">快捷工具</div>
        <h2>N度网络</h2>
        <p>选择一个博客和 N 值，点亮它在当前星图内 N 跳以内的关系网络。</p>
      </div>
      <button id="nd-close" class="nd-icon-btn" title="关闭" aria-label="关闭">×</button>
    </div>

    <div class="nd-scope" id="nd-scope"></div>

    <div class="nd-field">
      <label for="nd-node-input">中心博客</label>
      <input id="nd-node-input" type="text" autocomplete="off" placeholder="输入标题、域名或描述关键词" />
      <div class="nd-picked" id="nd-picked"></div>
      <div class="nd-suggest" id="nd-suggest"></div>
    </div>

    <div class="nd-degree-row">
      <label for="nd-degree-input">网络层数 N</label>
      <input id="nd-degree-input" type="number" min="1" max="3" step="1" value="1" inputmode="numeric" />
      <span>范围 1-3</span>
    </div>

    <div class="nd-actions">
      <button id="nd-run" class="nd-primary" disabled>点亮 N 度网络</button>
      <button id="nd-clear" class="nd-secondary">清除高亮</button>
    </div>

    <div id="nd-status" class="nd-status"></div>
    <div id="nd-result" class="nd-result"></div>
  `,document.body.appendChild(t),t.querySelector("#nd-close").onclick=h,t.querySelector("#nd-run").onclick=()=>{V()},t.querySelector("#nd-clear").onclick=()=>p(),A(),O(),document.addEventListener("mousedown",r=>{t&&!t.contains(r.target)&&b()}),t)}function j(){L||(L=!0,window.addEventListener($,r=>{r.detail!=="ndegree-network"&&h()}))}function A(){const r=x();r.addEventListener("input",()=>{e&&(e.selected=null,p(),m(),u(),N())}),r.addEventListener("focus",N),r.addEventListener("keydown",n=>{var d;const o=D().querySelector(".nd-suggest-item");if(n.key==="Enter"&&o){n.preventDefault();const i=(d=e==null?void 0:e.graph)==null?void 0:d.nodeById.get(o.dataset.id??"");i&&q(i)}n.key==="Escape"&&b()})}function O(){const r=w();r.addEventListener("input",()=>{u(),r.value&&!f(!1)&&l("N 必须是 1、2 或 3。","error")}),r.addEventListener("change",()=>{const n=f(!0);r.value=String(n),e&&(e.degree=n),p(),u()})}function U(r){return r.map(n=>({node:n,host:I(n.url),haystack:`${n.title} ${n.url} ${n.description??""} ${z(n)}`.toLowerCase()}))}function E(){var d,i;const r=t==null?void 0:t.querySelector("#nd-scope");if(!r)return;const n=((d=e==null?void 0:e.graph)==null?void 0:d.nodes.length)??0,o=((i=e==null?void 0:e.graph)==null?void 0:i.edges.length)??0;r.textContent=`${(e==null?void 0:e.scopeLabel)??"当前星图"} · ${n.toLocaleString("zh-CN")} 个博客 · ${o.toLocaleString("zh-CN")} 条关系`}function m(){if(!t||!e)return;const r=t.querySelector("#nd-picked");if(!e.selected){r.innerHTML="";return}x().value=e.selected.title||e.selected.url,r.innerHTML=`
    <div class="nd-picked-title">${c(e.selected.title||e.selected.url)}</div>
    <div class="nd-picked-url">${c(e.selected.url)}</div>
  `}function N(){if(!e)return;const r=x(),n=D(),o=r.value.trim().toLowerCase();n.innerHTML="";const d=e.selected?e.selected.title||e.selected.url:"";if(!o||d===r.value){n.classList.remove("open");return}const i=F(o);if(!i.length){n.innerHTML='<div class="nd-suggest-empty">没有匹配的博客</div>',n.classList.add("open");return}n.innerHTML=i.map(({node:a,host:s})=>`
    <button class="nd-suggest-item" data-id="${T(a.id)}">
      <span class="nd-si-title">${c(a.title||a.url)}</span>
      <span class="nd-si-url">${c(s||a.url)}</span>
      <span class="nd-si-cat">${c(z(a))}</span>
    </button>
  `).join(""),n.querySelectorAll(".nd-suggest-item").forEach(a=>{a.onmousedown=s=>{var y;s.preventDefault();const v=(y=e==null?void 0:e.graph)==null?void 0:y.nodeById.get(a.dataset.id??"");v&&q(v)}}),n.classList.add("open")}function F(r){if(!e)return[];const n=[];for(const o of e.rows){const d=o.node.title.toLowerCase(),i=o.node.url.toLowerCase(),a=o.host.toLowerCase();let s=99;d.startsWith(r)?s=0:a.startsWith(r)?s=1:d.includes(r)?s=2:a.includes(r)?s=3:i.includes(r)?s=4:o.haystack.includes(r)&&(s=5),s<99&&n.push({...o,score:s})}return n.sort((o,d)=>o.score-d.score||d.node.inDegree+d.node.outDegree-(o.node.inDegree+o.node.outDegree)),n.slice(0,8)}function q(r){e&&(e.selected=r,m(),b(),p(),u(),l("","muted"))}async function V(){if(!(e!=null&&e.graph)||!e.selected||e.busy)return;const r=f(!1);if(!r){l("N 必须是 1、2 或 3。","error");return}e.degree=r,S(!0),l(`正在计算 ${r} 度网络...`,"loading"),await new Promise(n=>requestAnimationFrame(n));try{const n=await P(e.graph,e.selected.id,r);if(e.lastResult=n,!n){e.callbacks.onClear(),l("当前星图里没有找到这个博客。","error"),g(null);return}e.callbacks.onNetwork(n.nodeIds,n.edgePairs,e.selected.id),l(`已点亮 ${n.nodeIds.length.toLocaleString("zh-CN")} 个节点、${n.edgePairs.length.toLocaleString("zh-CN")} 条联系 · ${Math.max(1,Math.round(n.durationMs))}ms`,"success"),g(n)}catch(n){e.callbacks.onClear(),l(n.message||"N 度网络计算失败","error"),g(null)}finally{S(!1)}}function g(r){const n=t==null?void 0:t.querySelector("#nd-result");if(!n||!(e!=null&&e.graph))return;if(!r){n.innerHTML="";return}const o=r.layers.map((i,a)=>({idx:a,count:i})).filter(({count:i})=>i>0),d=r.nodeIds.slice(0,24).map(i=>e.graph.nodeById.get(i)).filter(Boolean);n.innerHTML=`
    <div class="nd-result-title">层级统计</div>
    <div class="nd-layer-list">
      ${o.map(({idx:i,count:a})=>`
        <div class="nd-layer-item">
          <span>${i===0?"中心":`${i} 度`}</span>
          <strong>${a.toLocaleString("zh-CN")}</strong>
        </div>
      `).join("")}
    </div>
    <div class="nd-result-title" style="margin-top:12px">节点预览</div>
    <div class="nd-node-list">
      ${d.map(i=>`
        <button class="nd-node-item" data-id="${T(i.id)}">
          <span class="nd-node-title">${c(i.title||i.url)}</span>
          <span class="nd-node-url">${c(I(i.url)||i.url)}</span>
        </button>
      `).join("")}
    </div>
  `,n.querySelectorAll(".nd-node-item").forEach(i=>{i.onclick=()=>{const a=i.dataset.id;a&&(e==null||e.callbacks.onFocusNode(a))}})}function p(r=!0){e&&(e.lastResult=null,r&&e.callbacks.onClear(),l("","muted"),g(null))}function f(r){const n=w(),o=Number(n.value);return Number.isInteger(o)&&o>=1&&o<=3?o:r?Math.max(1,Math.min(3,Math.round(Number.isFinite(o)?o:1))):null}function S(r){!e||!t||(e.busy=r,t.classList.toggle("busy",r),x().disabled=r,w().disabled=r,t.querySelector("#nd-run").disabled=r||!M())}function u(){t&&(t.querySelector("#nd-run").disabled=!M())}function M(){return!!(e!=null&&e.graph&&e.selected&&f(!1)&&!e.busy)}function l(r,n){const o=t==null?void 0:t.querySelector("#nd-status");o&&(o.textContent=r,o.className=`nd-status ${r?"show":""} ${n}`)}function b(){var r;(r=t==null?void 0:t.querySelector(".nd-suggest"))==null||r.classList.remove("open")}function x(){return t.querySelector("#nd-node-input")}function w(){return t.querySelector("#nd-degree-input")}function D(){return t.querySelector("#nd-suggest")}function I(r){try{return new URL(r).hostname.replace(/^www\./,"")}catch{return r}}function c(r){return String(r??"").replace(/[&<>"']/g,n=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[n])}function T(r){return c(r).replace(/\s/g," ")}function W(){if(document.getElementById("ndegree-style"))return;const r=document.createElement("style");r.id="ndegree-style",r.textContent=`
    #ndegree-panel {
      position: fixed;
      top: 78px;
      right: 24px;
      width: 370px;
      max-width: calc(100vw - 32px);
      max-height: calc(100vh - 110px);
      overflow: auto;
      z-index: 14;
      display: none;
      padding: 18px;
      border: 1px solid rgba(100,200,255,0.22);
      border-radius: 16px;
      background:
        radial-gradient(circle at 12% 0%, rgba(111,214,155,0.13), transparent 34%),
        rgba(4,12,35,0.9);
      backdrop-filter: blur(18px);
      box-shadow: 0 16px 70px rgba(0,0,0,0.44), 0 0 50px rgba(0,160,255,0.08);
      color: #c8e8f5;
    }
    #ndegree-panel.open { display: block; animation: nd-in 0.22s ease-out; }
    @keyframes nd-in { from { opacity: 0; transform: translateY(-8px); } to { opacity: 1; transform: none; } }
    #ndegree-panel::-webkit-scrollbar { width: 4px; }
    #ndegree-panel::-webkit-scrollbar-thumb { background: rgba(100,200,255,0.22); border-radius: 3px; }
    .nd-head { display: flex; align-items: flex-start; gap: 12px; margin-bottom: 12px; }
    .nd-head h2 { margin: 0 0 5px; color: #e0f7ff; font-size: 1.05rem; font-weight: 600; letter-spacing: 0.02em; }
    .nd-head p { margin: 0; color: #6f9db2; font-size: 0.78rem; line-height: 1.55; }
    .nd-kicker { color: #6fd69b; font-size: 0.66rem; letter-spacing: 0.14em; margin-bottom: 5px; }
    .nd-icon-btn {
      margin-left: auto;
      width: 28px;
      height: 28px;
      border-radius: 8px;
      border: 1px solid rgba(100,200,255,0.12);
      background: rgba(0,40,90,0.12);
      color: #5d8ba4;
      cursor: pointer;
      font-size: 1.05rem;
    }
    .nd-icon-btn:hover { color: #d8f8ff; border-color: rgba(100,200,255,0.35); }
    .nd-scope {
      margin-bottom: 14px;
      padding: 8px 10px;
      border-radius: 10px;
      background: rgba(0,120,200,0.08);
      border: 1px solid rgba(100,200,255,0.12);
      color: #78abc2;
      font-size: 0.72rem;
      line-height: 1.45;
    }
    .nd-field { position: relative; min-width: 0; margin-bottom: 10px; }
    .nd-field label, .nd-degree-row label {
      display: block;
      margin-bottom: 6px;
      color: #7aaec5;
      font-size: 0.74rem;
      letter-spacing: 0.04em;
    }
    .nd-field input, .nd-degree-row input {
      width: 100%;
      padding: 10px 12px;
      border-radius: 9px;
      border: 1px solid rgba(100,200,255,0.2);
      background: rgba(2,10,28,0.62);
      color: #d8f2ff;
      outline: none;
      font-size: 0.86rem;
      font-family: inherit;
    }
    .nd-field input:focus, .nd-degree-row input:focus { border-color: rgba(111,214,155,0.5); }
    .nd-field input::placeholder { color: #315b73; }
    .nd-degree-row {
      display: grid;
      grid-template-columns: minmax(0, 1fr) 82px;
      gap: 6px 12px;
      align-items: end;
      margin-top: 8px;
    }
    .nd-degree-row label { grid-column: 1 / 2; margin-bottom: 0; }
    .nd-degree-row input { grid-column: 2 / 3; text-align: center; }
    .nd-degree-row span {
      grid-column: 1 / 2;
      grid-row: 2 / 3;
      color: #4f819a;
      font-size: 0.7rem;
    }
    .nd-picked { min-height: 0; margin-top: 7px; color: #8fbdd0; }
    .nd-picked-title {
      color: #e0f7ff;
      font-size: 0.78rem;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .nd-picked-url {
      color: #3f7f9a;
      font-size: 0.68rem;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      margin-top: 2px;
    }
    .nd-suggest {
      display: none;
      position: absolute;
      left: 0;
      right: 0;
      top: calc(100% + 4px);
      z-index: 2;
      overflow: auto;
      max-height: 248px;
      border: 1px solid rgba(100,200,255,0.2);
      border-radius: 10px;
      background: rgba(3,10,28,0.96);
      backdrop-filter: blur(16px);
      box-shadow: 0 10px 30px rgba(0,0,0,0.36);
    }
    .nd-suggest.open { display: block; }
    .nd-suggest-item {
      width: 100%;
      display: grid;
      grid-template-columns: minmax(0, 1fr) auto;
      gap: 3px 8px;
      padding: 9px 11px;
      border: 0;
      border-bottom: 1px solid rgba(100,200,255,0.07);
      background: transparent;
      color: inherit;
      text-align: left;
      cursor: pointer;
      font-family: inherit;
    }
    .nd-suggest-item:hover { background: rgba(0,160,255,0.13); }
    .nd-si-title {
      color: #d6f3ff;
      font-size: 0.78rem;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .nd-si-url {
      grid-column: 1 / 2;
      color: #3e7894;
      font-size: 0.66rem;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .nd-si-cat {
      grid-row: 1 / 3;
      grid-column: 2 / 3;
      align-self: center;
      padding: 2px 7px;
      border-radius: 999px;
      border: 1px solid rgba(111,214,155,0.18);
      color: #76cf9a;
      background: rgba(111,214,155,0.06);
      font-size: 0.62rem;
      max-width: 82px;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .nd-suggest-empty { padding: 12px; color: #4f7a90; font-size: 0.78rem; }
    .nd-actions { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 9px; margin-top: 14px; }
    .nd-primary, .nd-secondary {
      border-radius: 10px;
      padding: 10px 12px;
      font-family: inherit;
      cursor: pointer;
      transition: all 0.18s;
    }
    .nd-primary {
      border: 1px solid rgba(111,214,155,0.42);
      background: linear-gradient(135deg, rgba(60,200,120,0.2), rgba(0,140,180,0.14));
      color: #b9f7d0;
      font-size: 0.86rem;
    }
    .nd-primary:hover:not(:disabled) { box-shadow: 0 0 22px rgba(111,214,155,0.14); color: #e5ffef; }
    .nd-primary:disabled { opacity: 0.42; cursor: default; }
    .nd-secondary {
      border: 1px solid rgba(100,200,255,0.14);
      background: rgba(0,40,90,0.1);
      color: #6798af;
      font-size: 0.78rem;
    }
    .nd-secondary:hover { color: #a9edff; border-color: rgba(100,200,255,0.34); }
    .nd-status {
      display: none;
      margin-top: 10px;
      padding: 9px 10px;
      border-radius: 9px;
      font-size: 0.76rem;
      line-height: 1.5;
    }
    .nd-status.show { display: block; }
    .nd-status.loading, .nd-status.muted {
      color: #80b3ca;
      background: rgba(100,200,255,0.07);
      border: 1px solid rgba(100,200,255,0.13);
    }
    .nd-status.success {
      color: #9ddfbd;
      background: rgba(111,214,155,0.08);
      border: 1px solid rgba(111,214,155,0.2);
    }
    .nd-status.error {
      color: #ffb1a8;
      background: rgba(255,100,100,0.09);
      border: 1px solid rgba(255,100,100,0.22);
    }
    .nd-result { margin-top: 12px; }
    .nd-result-title {
      margin-bottom: 8px;
      color: #78abc2;
      font-size: 0.72rem;
      letter-spacing: 0.08em;
    }
    .nd-layer-list { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 7px; }
    .nd-layer-item {
      display: flex;
      align-items: center;
      justify-content: space-between;
      padding: 7px 9px;
      border-radius: 9px;
      background: rgba(111,214,155,0.07);
      border: 1px solid rgba(111,214,155,0.12);
      color: #89bda0;
      font-size: 0.72rem;
    }
    .nd-layer-item strong { color: #dfffea; font-size: 0.82rem; }
    .nd-node-list { display: grid; gap: 7px; }
    .nd-node-item {
      width: 100%;
      display: block;
      border: 1px solid rgba(111,214,155,0.15);
      border-radius: 10px;
      background: rgba(111,214,155,0.045);
      padding: 8px 9px;
      cursor: pointer;
      font-family: inherit;
      text-align: left;
    }
    .nd-node-item:hover { border-color: rgba(111,214,155,0.34); background: rgba(111,214,155,0.08); }
    .nd-node-title, .nd-node-url {
      display: block;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .nd-node-title { color: #f4fbff; font-size: 0.78rem; }
    .nd-node-url { color: #7f9c94; font-size: 0.66rem; margin-top: 2px; }
    #ndegree-panel.busy .nd-primary::after {
      content: '';
      display: inline-block;
      width: 10px;
      height: 10px;
      margin-left: 8px;
      border: 2px solid rgba(185,247,208,0.26);
      border-top-color: #b9f7d0;
      border-radius: 50%;
      vertical-align: -1px;
      animation: nd-spin 0.8s linear infinite;
    }
    @keyframes nd-spin { to { transform: rotate(360deg); } }
    @media (max-width: 760px) {
      #ndegree-panel {
        top: auto;
        right: 12px;
        bottom: 12px;
        left: 12px;
        width: auto;
        max-width: none;
        max-height: min(74vh, 620px);
        padding: 16px;
      }
      .nd-head h2 { font-size: 1rem; }
      .nd-head p { font-size: 0.76rem; }
      .nd-actions { grid-template-columns: 1fr; }
      .nd-secondary { width: 100%; }
      .nd-layer-list { grid-template-columns: 1fr; }
    }
  `,document.head.appendChild(r)}export{K as bootNDegreeNetwork,Q as openNDegreeNetwork};
