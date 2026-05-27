import{o as I,g as D,a as E}from"./index-DB71QehS.js";import{b as A,f as F}from"./pathfinder-wgwU7SjB.js";import"./three-Df2n1lyU.js";import"./postprocessing-D4VCwszR.js";let e=null,o=null,k=null,L=!1;const q="meetblog:tool-open";function Q(t){return e={graph:null,rows:[],scopeLabel:"当前星图",from:null,to:null,busy:!1,callbacks:t,lastResult:null},P(),k||(k=I(r=>{r||v()})),{setView(r,n,i){e&&(e.graph=A(r,n),e.rows=O(r),e.scopeLabel=i,e.from&&!e.graph.nodeById.has(e.from.id)&&(e.from=null),e.to&&!e.graph.nodeById.has(e.to.id)&&(e.to=null),c(!1),g(),T(),m())},clearPath(){c()}}}function X(){if(P(),!D())return;window.dispatchEvent(new CustomEvent(q,{detail:"path-finder"})),o.classList.add("open"),T(),g(),d(e!=null&&e.graph?"":"星图仍在加载，稍后即可使用路径查找。","muted"),m();const t=o.querySelector(e!=null&&e.from?"#path-to-input":"#path-from-input");setTimeout(()=>t==null?void 0:t.focus(),40)}function v(){o==null||o.classList.remove("open"),h()}function P(){return _(),j(),o||(o=document.createElement("div"),o.id="path-finder-panel",o.innerHTML=`
    <div class="pf-head">
      <div>
        <div class="pf-kicker">注册用户工具</div>
        <h2>博客路径查找</h2>
        <p>选择两个博客，按友链关系连接计算当前星图内的最短路径，并在星图中点亮整条路径。</p>
      </div>
      <button id="pf-close" class="pf-icon-btn" title="关闭" aria-label="关闭">×</button>
    </div>

    <div class="pf-scope" id="pf-scope"></div>

    <div class="pf-fields">
      <div class="pf-field" data-side="from">
        <label for="path-from-input">起点博客</label>
        <input id="path-from-input" type="text" autocomplete="off" placeholder="输入标题、域名或描述关键词" />
        <div class="pf-picked" id="pf-from-picked"></div>
        <div class="pf-suggest" id="pf-from-suggest"></div>
      </div>

      <button class="pf-swap" id="pf-swap" title="交换起点和终点">⇄</button>

      <div class="pf-field" data-side="to">
        <label for="path-to-input">终点博客</label>
        <input id="path-to-input" type="text" autocomplete="off" placeholder="输入另一个博客关键词" />
        <div class="pf-picked" id="pf-to-picked"></div>
        <div class="pf-suggest" id="pf-to-suggest"></div>
      </div>
    </div>

    <div class="pf-actions">
      <button id="pf-run" class="pf-primary" disabled>点亮最短路径</button>
      <button id="pf-clear" class="pf-secondary">清除高亮</button>
    </div>

    <div id="pf-status" class="pf-status"></div>
    <div id="pf-result" class="pf-result"></div>
  `,document.body.appendChild(o),o.querySelector("#pf-close").onclick=v,o.querySelector("#pf-run").onclick=()=>{V()},o.querySelector("#pf-clear").onclick=()=>c(),o.querySelector("#pf-swap").onclick=W,S("from"),S("to"),document.addEventListener("mousedown",t=>{o&&!o.contains(t.target)&&h()}),o)}function j(){L||(L=!0,window.addEventListener(q,t=>{t.detail!=="path-finder"&&v()}))}function S(t){const r=p(t);r.addEventListener("input",()=>{e&&(e[t]=null,c(),g(),m(),$(t))}),r.addEventListener("focus",()=>$(t)),r.addEventListener("keydown",n=>{var a;const i=H(t).querySelector(".pf-suggest-item");if(n.key==="Enter"&&i){n.preventDefault();const l=(a=e==null?void 0:e.graph)==null?void 0:a.nodeById.get(i.dataset.id??"");l&&M(t,l)}n.key==="Escape"&&h()})}function O(t){return t.map(r=>({node:r,host:N(r.url),haystack:`${r.title} ${r.url} ${r.description??""} ${E(r)}`.toLowerCase()}))}function T(){var i,a;const t=o==null?void 0:o.querySelector("#pf-scope");if(!t)return;const r=((i=e==null?void 0:e.graph)==null?void 0:i.nodes.length)??0,n=((a=e==null?void 0:e.graph)==null?void 0:a.edges.length)??0;t.textContent=`${(e==null?void 0:e.scopeLabel)??"当前星图"} · ${r.toLocaleString("zh-CN")} 个博客 · ${n.toLocaleString("zh-CN")} 条关系`}function g(){!o||!e||(z("from",e.from),z("to",e.to))}function z(t,r){const n=p(t),i=o.querySelector(t==="from"?"#pf-from-picked":"#pf-to-picked");if(!r){i.innerHTML="";return}n.value=r.title||r.url,i.innerHTML=`
    <div class="pf-picked-title">${f(r.title||r.url)}</div>
    <div class="pf-picked-url">${f(r.url)}</div>
  `}function $(t){if(!e)return;const r=p(t),n=H(t),i=r.value.trim().toLowerCase();n.innerHTML="";const a=e[t]?e[t].title||e[t].url:"";if(!i||a===r.value){n.classList.remove("open");return}const l=t==="from"?e.to:e.from,u=U(i,l==null?void 0:l.id);if(!u.length){n.innerHTML='<div class="pf-suggest-empty">没有匹配的博客</div>',n.classList.add("open");return}n.innerHTML=u.map(({node:s,host:x})=>`
    <button class="pf-suggest-item" data-id="${R(s.id)}">
      <span class="pf-si-title">${f(s.title||s.url)}</span>
      <span class="pf-si-url">${f(x||s.url)}</span>
      <span class="pf-si-cat">${f(E(s))}</span>
    </button>
  `).join(""),n.querySelectorAll(".pf-suggest-item").forEach(s=>{s.onmousedown=x=>{var y;x.preventDefault();const w=(y=e==null?void 0:e.graph)==null?void 0:y.nodeById.get(s.dataset.id??"");w&&M(t,w)}}),n.classList.add("open")}function U(t,r){if(!e)return[];const n=[];for(const i of e.rows){if(i.node.id===r)continue;const a=i.node.title.toLowerCase(),l=i.node.url.toLowerCase(),u=i.host.toLowerCase();let s=99;a.startsWith(t)?s=0:u.startsWith(t)?s=1:a.includes(t)?s=2:u.includes(t)?s=3:l.includes(t)?s=4:i.haystack.includes(t)&&(s=5),s<99&&n.push({...i,score:s})}return n.sort((i,a)=>i.score-a.score||a.node.inDegree+a.node.outDegree-(i.node.inDegree+i.node.outDegree)),n.slice(0,8)}function M(t,r){if(!e)return;e[t]=r,g(),h(),m(),c();const n=p(t==="from"?"to":"from");e[t==="from"?"to":"from"]||n.focus()}async function V(){if(!(!(e!=null&&e.graph)||!e.from||!e.to||e.busy)){if(e.from.id===e.to.id){d("请选择两个不同的博客。","error");return}C(!0),d("正在计算最短路径...","loading"),await new Promise(t=>requestAnimationFrame(t));try{const t=await F(e.graph,e.from.id,e.to.id);if(e.lastResult=t,!t){e.callbacks.onClear(),d("当前星图范围内没有找到连通路径。可切换到全部分类，或用更高探索深度重新进入星图后再试。","error"),b([]);return}e.callbacks.onPath(t.path),d(`已点亮 ${Math.max(0,t.path.length-1)} 跳路径 · 访问 ${t.visited.toLocaleString("zh-CN")} 个候选节点 · ${Math.max(1,Math.round(t.durationMs))}ms`,"success"),b(t.path)}catch(t){e.callbacks.onClear(),d(t.message||"路径计算失败","error"),b([])}finally{C(!1)}}}function b(t){const r=o==null?void 0:o.querySelector("#pf-result");if(!r||!(e!=null&&e.graph))return;if(!t.length){r.innerHTML="";return}const n=t.map(i=>e.graph.nodeById.get(i)).filter(Boolean);r.innerHTML=`
    <div class="pf-result-title">路径节点</div>
    <div class="pf-path-list">
      ${n.map((i,a)=>`
        <button class="pf-path-step" data-id="${R(i.id)}">
          <span class="pf-step-no">${a+1}</span>
          <span class="pf-step-main">
            <span class="pf-step-title">${f(i.title||i.url)}</span>
            <span class="pf-step-url">${f(N(i.url)||i.url)}</span>
          </span>
        </button>
      `).join("")}
    </div>
  `,r.querySelectorAll(".pf-path-step").forEach(i=>{i.onclick=()=>{const a=i.dataset.id;a&&(e==null||e.callbacks.onFocusNode(a))}})}function c(t=!0){e&&(e.lastResult=null,t&&e.callbacks.onClear(),d("","muted"),b([]))}function W(){if(!e)return;const t=e.from;e.from=e.to,e.to=t;const r=p("from").value;p("from").value=p("to").value,p("to").value=r,g(),m(),c()}function C(t){!e||!o||(e.busy=t,o.classList.toggle("busy",t),p("from").disabled=t,p("to").disabled=t,o.querySelector("#pf-run").disabled=t||!B())}function m(){o&&(o.querySelector("#pf-run").disabled=!B())}function B(){return!!(e!=null&&e.graph&&e.from&&e.to&&e.from.id!==e.to.id&&!e.busy)}function d(t,r){const n=o==null?void 0:o.querySelector("#pf-status");n&&(n.textContent=t,n.className=`pf-status ${t?"show":""} ${r}`)}function h(){o==null||o.querySelectorAll(".pf-suggest").forEach(t=>t.classList.remove("open"))}function p(t){return o.querySelector(t==="from"?"#path-from-input":"#path-to-input")}function H(t){return o.querySelector(t==="from"?"#pf-from-suggest":"#pf-to-suggest")}function N(t){try{return new URL(t).hostname.replace(/^www\./,"")}catch{return t}}function f(t){return String(t??"").replace(/[&<>"']/g,r=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[r])}function R(t){return f(t).replace(/\s/g," ")}function _(){if(document.getElementById("path-finder-style"))return;const t=document.createElement("style");t.id="path-finder-style",t.textContent=`
    #path-finder-panel {
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
        radial-gradient(circle at 10% 0%, rgba(125,248,255,0.12), transparent 34%),
        rgba(4,12,35,0.9);
      backdrop-filter: blur(18px);
      box-shadow: 0 16px 70px rgba(0,0,0,0.44), 0 0 50px rgba(0,160,255,0.08);
      color: #c8e8f5;
    }
    #path-finder-panel.open { display: block; animation: pf-in 0.22s ease-out; }
    @keyframes pf-in { from { opacity: 0; transform: translateY(-8px); } to { opacity: 1; transform: none; } }
    #path-finder-panel::-webkit-scrollbar { width: 4px; }
    #path-finder-panel::-webkit-scrollbar-thumb { background: rgba(100,200,255,0.22); border-radius: 3px; }
    .pf-head { display: flex; align-items: flex-start; gap: 12px; margin-bottom: 12px; }
    .pf-head h2 { margin: 0 0 5px; color: #e0f7ff; font-size: 1.05rem; font-weight: 600; letter-spacing: 0.02em; }
    .pf-head p { margin: 0; color: #6f9db2; font-size: 0.78rem; line-height: 1.55; }
    .pf-kicker { color: #7df8ff; font-size: 0.66rem; letter-spacing: 0.14em; margin-bottom: 5px; }
    .pf-icon-btn {
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
    .pf-icon-btn:hover { color: #d8f8ff; border-color: rgba(100,200,255,0.35); }
    .pf-scope {
      margin-bottom: 14px;
      padding: 8px 10px;
      border-radius: 10px;
      background: rgba(0,120,200,0.08);
      border: 1px solid rgba(100,200,255,0.12);
      color: #78abc2;
      font-size: 0.72rem;
      line-height: 1.45;
    }
    .pf-fields { display: grid; gap: 10px; }
    .pf-field { position: relative; min-width: 0; }
    .pf-field label {
      display: block;
      margin-bottom: 6px;
      color: #7aaec5;
      font-size: 0.74rem;
      letter-spacing: 0.04em;
    }
    .pf-field input {
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
    .pf-field input:focus { border-color: rgba(125,248,255,0.48); }
    .pf-field input::placeholder { color: #315b73; }
    .pf-picked {
      min-height: 0;
      margin-top: 7px;
      color: #8fbdd0;
    }
    .pf-picked-title {
      color: #e0f7ff;
      font-size: 0.78rem;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .pf-picked-url {
      color: #3f7f9a;
      font-size: 0.68rem;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      margin-top: 2px;
    }
    .pf-swap {
      justify-self: center;
      width: 34px;
      height: 30px;
      border-radius: 999px;
      border: 1px solid rgba(100,200,255,0.16);
      background: rgba(0,140,255,0.09);
      color: #77dff0;
      cursor: pointer;
      font-size: 1rem;
    }
    .pf-swap:hover { border-color: rgba(125,248,255,0.45); background: rgba(0,160,255,0.16); }
    .pf-suggest {
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
    .pf-suggest.open { display: block; }
    .pf-suggest-item {
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
    .pf-suggest-item:hover { background: rgba(0,160,255,0.13); }
    .pf-si-title {
      color: #d6f3ff;
      font-size: 0.78rem;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .pf-si-url {
      grid-column: 1 / 2;
      color: #3e7894;
      font-size: 0.66rem;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .pf-si-cat {
      grid-row: 1 / 3;
      grid-column: 2 / 3;
      align-self: center;
      padding: 2px 7px;
      border-radius: 999px;
      border: 1px solid rgba(0,200,255,0.16);
      color: #54b2d0;
      background: rgba(0,200,255,0.06);
      font-size: 0.62rem;
      max-width: 82px;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .pf-suggest-empty { padding: 12px; color: #4f7a90; font-size: 0.78rem; }
    .pf-actions { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 9px; margin-top: 14px; }
    .pf-primary, .pf-secondary {
      border-radius: 10px;
      padding: 10px 12px;
      font-family: inherit;
      cursor: pointer;
      transition: all 0.18s;
    }
    .pf-primary {
      border: 1px solid rgba(0,200,255,0.42);
      background: linear-gradient(135deg, rgba(0,180,255,0.22), rgba(0,100,200,0.16));
      color: #a9f2ff;
      font-size: 0.86rem;
    }
    .pf-primary:hover:not(:disabled) { box-shadow: 0 0 22px rgba(0,200,255,0.16); color: #e5fbff; }
    .pf-primary:disabled { opacity: 0.42; cursor: default; }
    .pf-secondary {
      border: 1px solid rgba(100,200,255,0.14);
      background: rgba(0,40,90,0.1);
      color: #6798af;
      font-size: 0.78rem;
    }
    .pf-secondary:hover { color: #a9edff; border-color: rgba(100,200,255,0.34); }
    .pf-status {
      display: none;
      margin-top: 10px;
      padding: 9px 10px;
      border-radius: 9px;
      font-size: 0.76rem;
      line-height: 1.5;
    }
    .pf-status.show { display: block; }
    .pf-status.loading, .pf-status.muted {
      color: #80b3ca;
      background: rgba(100,200,255,0.07);
      border: 1px solid rgba(100,200,255,0.13);
    }
    .pf-status.success {
      color: #9ddfbd;
      background: rgba(111,214,155,0.08);
      border: 1px solid rgba(111,214,155,0.2);
    }
    .pf-status.error {
      color: #ffb1a8;
      background: rgba(255,100,100,0.09);
      border: 1px solid rgba(255,100,100,0.22);
    }
    .pf-result { margin-top: 12px; }
    .pf-result-title {
      margin-bottom: 8px;
      color: #78abc2;
      font-size: 0.72rem;
      letter-spacing: 0.08em;
    }
    .pf-path-list { display: grid; gap: 7px; }
    .pf-path-step {
      display: grid;
      grid-template-columns: 24px minmax(0, 1fr);
      gap: 8px;
      align-items: center;
      width: 100%;
      border: 1px solid rgba(255,232,140,0.18);
      border-radius: 10px;
      background: rgba(255,210,90,0.055);
      padding: 8px 9px;
      cursor: pointer;
      font-family: inherit;
      text-align: left;
    }
    .pf-path-step:hover { border-color: rgba(255,232,140,0.36); background: rgba(255,210,90,0.09); }
    .pf-step-no {
      width: 24px;
      height: 24px;
      display: inline-flex;
      align-items: center;
      justify-content: center;
      border-radius: 999px;
      color: #201800;
      background: #fff0a5;
      font-size: 0.72rem;
      font-weight: 700;
    }
    .pf-step-main { min-width: 0; }
    .pf-step-title, .pf-step-url {
      display: block;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .pf-step-title { color: #f4fbff; font-size: 0.78rem; }
    .pf-step-url { color: #7f9c94; font-size: 0.66rem; margin-top: 2px; }
    #path-finder-panel.busy .pf-primary::after {
      content: '';
      display: inline-block;
      width: 10px;
      height: 10px;
      margin-left: 8px;
      border: 2px solid rgba(169,242,255,0.26);
      border-top-color: #a9f2ff;
      border-radius: 50%;
      vertical-align: -1px;
      animation: pf-spin 0.8s linear infinite;
    }
    @keyframes pf-spin { to { transform: rotate(360deg); } }
    @media (max-width: 760px) {
      #path-finder-panel {
        top: auto;
        right: 12px;
        bottom: 12px;
        left: 12px;
        width: auto;
        max-width: none;
        max-height: min(74vh, 620px);
        padding: 16px;
      }
      .pf-head h2 { font-size: 1rem; }
      .pf-head p { font-size: 0.76rem; }
      .pf-actions { grid-template-columns: 1fr; }
      .pf-secondary { width: 100%; }
    }
  `,document.head.appendChild(t)}export{Q as bootPathFinder,X as openPathFinder};
