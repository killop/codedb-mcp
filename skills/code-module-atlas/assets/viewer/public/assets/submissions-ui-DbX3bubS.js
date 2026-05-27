import{g as f}from"./index-DB71QehS.js";import"./three-Df2n1lyU.js";import"./postprocessing-D4VCwszR.js";const d={name:"Meet Blog 博客星图",url:"https://meet-blog.buyixiao.xyz/",icon:"https://meet-blog.buyixiao.xyz/favicon.svg",random:"https://meet-blog.buyixiao.xyz/?random=on",randomAI:"https://meet-blog.buyixiao.xyz/?random=on&category=AI"};function m(t,e){const o=new URL(t);return o.searchParams.set("ref",e),o.toString()}function h(t){const e=m(d.url,t),o=m(d.random,t),i=m(d.randomAI,t),a=`[${d.name}](${e}) - 发现中文独立博客的星图导航`,s=`<a href="${e}" target="_blank" rel="noopener">
  <img src="${d.icon}" alt="" width="16" height="16" />
  ${d.name}
</a>`,l=`网站名：${d.name}
网站地址：${e}
网站图标：${d.icon}

进阶用法：
${o} 每次打开都能随机访问一个网站。
${i} 每次打开都能随机访问一个 AI 相关的网站。`;return{url:e,random:o,randomAI:i,markdown:a,html:s,fullInfo:l}}async function p(t,e={}){const o=await fetch(t,{credentials:"include",headers:{"Content-Type":"application/json",...e.headers??{}},...e}),i=await o.json().catch(()=>({}));if(!o.ok)throw new Error(i.error??`HTTP ${o.status}`);return i}let n=null;function v(){return w(),n||(n=document.createElement("div"),n.className="mb-overlay",n.innerHTML='<div class="mb-card" style="width: 480px;"></div>',n.addEventListener("click",t=>{t.target===n&&c()}),document.body.appendChild(n)),n}function x(){requestAnimationFrame(()=>n.classList.add("open"))}function c(){n==null||n.classList.remove("open")}function w(){if(document.getElementById("mb-submission-style"))return;const t=document.createElement("style");t.id="mb-submission-style",t.textContent=`
    .mb-link-guide {
      margin-top: 18px;
      color: #9ec9dc;
    }
    .mb-guide-hero {
      position: relative;
      overflow: hidden;
      border: 1px solid rgba(100,200,255,0.2);
      background:
        radial-gradient(circle at 12% 0%, rgba(125,248,255,0.16), transparent 34%),
        linear-gradient(145deg, rgba(7,25,58,0.88), rgba(3,10,28,0.94));
      border-radius: 14px;
      padding: 18px 18px 16px;
      margin-bottom: 16px;
    }
    .mb-guide-kicker {
      font-size: 0.72rem;
      letter-spacing: 0.12em;
      text-transform: uppercase;
      color: #7df8ff;
      margin-bottom: 8px;
    }
    .mb-guide-title {
      font-size: 1.32rem;
      line-height: 1.25;
      color: #e7f8ff;
      font-weight: 600;
      margin-bottom: 8px;
    }
    .mb-guide-copy {
      font-size: 0.86rem;
      line-height: 1.72;
      color: #8bb8cc;
      max-width: 58em;
    }
    .mb-guide-note {
      margin-top: 12px;
      padding: 10px 12px;
      border-radius: 10px;
      background: rgba(111,214,155,0.08);
      border: 1px solid rgba(111,214,155,0.2);
      color: #9ddfbd;
      font-size: 0.78rem;
      line-height: 1.55;
    }
    .mb-guide-grid {
      display: grid;
      grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
      gap: 12px;
      margin-bottom: 14px;
    }
    .mb-guide-panel {
      border: 1px solid rgba(100,200,255,0.14);
      background: rgba(5,15,40,0.46);
      border-radius: 12px;
      padding: 14px;
      min-width: 0;
    }
    .mb-guide-panel-title {
      color: #d8f0ff;
      font-size: 0.86rem;
      font-weight: 600;
      margin-bottom: 10px;
    }
    .mb-meta-row {
      display: grid;
      grid-template-columns: 72px minmax(0, 1fr);
      gap: 10px;
      align-items: start;
      font-size: 0.78rem;
      line-height: 1.55;
      padding: 6px 0;
      border-bottom: 1px solid rgba(100,200,255,0.08);
    }
    .mb-meta-row:last-child { border-bottom: 0; }
    .mb-meta-label { color: #4f819a; }
    .mb-meta-value {
      color: #cdefff;
      word-break: break-all;
      font-family: 'SF Mono', Menlo, Consolas, monospace;
      font-size: 0.75rem;
    }
    .mb-code-block {
      margin: 10px 0 8px;
      padding: 11px 12px;
      border-radius: 10px;
      border: 1px solid rgba(100,200,255,0.12);
      background: rgba(0,8,22,0.52);
      color: #bfeaff;
      font-family: 'SF Mono', Menlo, Consolas, monospace;
      font-size: 0.72rem;
      line-height: 1.55;
      white-space: pre-wrap;
      word-break: break-word;
    }
    .mb-guide-actions {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      margin-top: 12px;
    }
    .mb-copy-btn {
      border: 1px solid rgba(100,200,255,0.24);
      background: rgba(0,140,255,0.1);
      color: #8bdfff;
      border-radius: 8px;
      padding: 7px 10px;
      font-size: 0.76rem;
      cursor: pointer;
      font-family: inherit;
      transition: all 0.18s;
    }
    .mb-copy-btn:hover {
      border-color: rgba(125,248,255,0.48);
      background: rgba(0,170,255,0.18);
      color: #e2fbff;
    }
    .mb-copy-status {
      min-height: 20px;
      color: #6fd69b;
      font-size: 0.76rem;
      margin-top: 4px;
    }
    .mb-advanced-list {
      display: grid;
      gap: 8px;
      margin-top: 8px;
    }
    .mb-advanced-item {
      padding: 10px 11px;
      border-radius: 10px;
      background: rgba(125,248,255,0.05);
      border: 1px solid rgba(125,248,255,0.1);
    }
    .mb-advanced-desc {
      color: #8bb8cc;
      font-size: 0.76rem;
      line-height: 1.45;
      margin-bottom: 5px;
    }
    .mb-advanced-url {
      color: #7df8ff;
      font-family: 'SF Mono', Menlo, Consolas, monospace;
      font-size: 0.72rem;
      word-break: break-all;
    }
    .mb-guide-footer {
      display: flex;
      gap: 10px;
      align-items: center;
      margin-top: 16px;
    }
    .mb-guide-footer .mb-btn { flex: 1; }
    .mb-guide-footer .mb-btn-link { width: auto; margin-top: 0; white-space: nowrap; }
    @media (max-width: 760px) {
      .mb-guide-grid { grid-template-columns: 1fr; }
      .mb-guide-title { font-size: 1.12rem; }
      .mb-guide-footer { flex-direction: column; align-items: stretch; }
      .mb-guide-footer .mb-btn-link { width: 100%; }
    }
  `,document.head.appendChild(t)}function y(){if(!f())return;const e=v().querySelector(".mb-card");e.style.width="480px",e.style.maxHeight="",e.style.overflowY="",e.innerHTML=`
    <button class="mb-card-close" aria-label="关闭">✕</button>
    <h3>＋ 提交博客</h3>
    <p class="mb-card-sub">推荐一个你喜欢的中文独立博客，系统会自动检测可达性并抓取标题、描述供审核员核实。每日限 10 次。</p>

    <div class="mb-field">
      <label class="mb-label" for="mb-sub-url">博客 URL</label>
      <input id="mb-sub-url" class="mb-input" type="url" placeholder="https://blog.example.com" />
    </div>

    <div class="mb-field">
      <label class="mb-label" for="mb-sub-note">备注给审核员（可选）</label>
      <input id="mb-sub-note" class="mb-input" type="text" maxlength="500" placeholder="例如：自己的博客 / 朋友推荐" />
    </div>

    <button class="mb-btn" id="mb-sub-submit">提交审核</button>
    <div class="mb-error" id="mb-sub-err"></div>
    <div class="mb-info" id="mb-sub-info"></div>
  `,e.querySelector(".mb-card-close").onclick=c;const o=e.querySelector("#mb-sub-url"),i=e.querySelector("#mb-sub-note"),a=e.querySelector("#mb-sub-submit"),s=e.querySelector("#mb-sub-err"),l=e.querySelector("#mb-sub-info");o.focus(),a.onclick=async()=>{s.classList.remove("show"),l.classList.remove("show");const u=o.value.trim();if(!u){s.textContent="请输入博客 URL",s.classList.add("show");return}a.disabled=!0,a.textContent="正在检测博客可达性...";try{const b=await p("/api/submissions",{method:"POST",body:JSON.stringify({url:u,note:i.value.trim()||void 0})});k(e,b)}catch(b){s.textContent=b.message,s.classList.add("show"),a.disabled=!1,a.textContent="提交审核"}},x()}function k(t,e){const o=h(e.url);t.style.width="720px",t.style.maxHeight="calc(100vh - 32px)",t.style.overflowY="auto",t.innerHTML=`
    <button class="mb-card-close" aria-label="关闭">✕</button>
    <h3>提交成功</h3>
    <p class="mb-card-sub">审核员会尽快处理，审核结果将发送到您的注册邮箱。</p>

    <div class="mb-guide-hero">
      <div class="mb-guide-kicker">友情链接说明书</div>
      <div class="mb-guide-title">谢谢你把博客带到 Meet Blog，也欢迎把这片星图放进你的友链页。</div>
      <div class="mb-guide-copy">
        这不是审核条件，也不会影响你的提交结果。只是如果你的博客有友情链接、导航页或关于页，可以用下面的信息添加 Meet Blog。
        我们希望互相推荐的入口足够清楚、克制、真实，让读者更容易在中文独立博客之间继续发现值得读的人和文章。
      </div>
      <div class="mb-guide-note">你的提交已经进入审核队列。添加友情链接完全自愿；无论是否添加，我们都会按同一标准认真处理。</div>
    </div>

    <div style="background:rgba(0,200,255,0.05);border:1px solid rgba(100,200,255,0.14);border-radius:12px;padding:13px;margin-bottom:14px">
      <div style="display:flex;gap:12px;align-items:flex-start">
        <img src="${g(e.iconUrl||"")}" onerror="this.style.visibility='hidden'" style="width:38px;height:38px;border-radius:8px;background:rgba(0,100,200,0.15);flex-shrink:0;object-fit:cover;border:1px solid rgba(100,200,255,0.15)" />
        <div style="flex:1;min-width:0">
          <div style="font-size:0.72rem;color:#4a7a95;margin-bottom:3px">本次提交</div>
          <div style="font-size:0.9rem;color:#d8f0ff;font-weight:500;word-break:break-all">${r(e.title||"(未抓取到标题)")}</div>
          <div style="font-size:0.75rem;color:#3a8ab0;margin-top:4px;word-break:break-all">${r(e.url)}</div>
          ${e.description?`<div style="font-size:0.78rem;color:#5a8a9f;margin-top:8px;line-height:1.5">${r(e.description)}</div>`:""}
        </div>
      </div>
    </div>

    <div class="mb-link-guide">
      <div class="mb-guide-grid">
        <div class="mb-guide-panel">
          <div class="mb-guide-panel-title">基础信息</div>
          <div class="mb-meta-row"><div class="mb-meta-label">网站名</div><div class="mb-meta-value">${r(d.name)}</div></div>
          <div class="mb-meta-row"><div class="mb-meta-label">地址</div><div class="mb-meta-value">${r(o.url)}</div></div>
          <div class="mb-meta-row"><div class="mb-meta-label">图标</div><div class="mb-meta-value">${r(d.icon)}</div></div>
          <div class="mb-guide-actions">
            <button class="mb-copy-btn" data-copy="full">复制完整信息</button>
            <button class="mb-copy-btn" data-copy="markdown">复制 Markdown</button>
          </div>
        </div>

        <div class="mb-guide-panel">
          <div class="mb-guide-panel-title">HTML 友链示例</div>
          <div class="mb-code-block">${r(o.html)}</div>
          <div class="mb-guide-actions">
            <button class="mb-copy-btn" data-copy="html">复制 HTML</button>
            <a class="mb-copy-btn" href="${g(o.url)}" target="_blank" rel="noopener" style="text-decoration:none;display:inline-block">打开网站</a>
          </div>
        </div>
      </div>

      <div class="mb-guide-panel">
        <div class="mb-guide-panel-title">进阶用法</div>
        <div class="mb-advanced-list">
          <div class="mb-advanced-item">
            <div class="mb-advanced-desc">每次打开都随机访问一个网站，适合放在“随便逛逛”“随机博客”入口。</div>
            <div class="mb-advanced-url">${r(o.random)}</div>
          </div>
          <div class="mb-advanced-item">
            <div class="mb-advanced-desc">每次打开都随机访问一个 AI 相关的网站，适合主题导航或专题友链。</div>
            <div class="mb-advanced-url">${r(o.randomAI)}</div>
          </div>
        </div>
        <div class="mb-guide-actions">
          <button class="mb-copy-btn" data-copy="random">复制随机访问链接</button>
          <button class="mb-copy-btn" data-copy="random-ai">复制 AI 随机链接</button>
        </div>
      </div>

      <div class="mb-copy-status" id="mb-copy-status"></div>
    </div>

    <div class="mb-guide-footer">
      <button class="mb-btn" id="mb-sub-done">完成</button>
      <button class="mb-btn-link" id="mb-sub-more">继续提交另一个</button>
    </div>
  `,t.querySelector(".mb-card-close").onclick=c,t.querySelector("#mb-sub-done").onclick=c,t.querySelector("#mb-sub-more").onclick=y,$(t,o)}function $(t,e){const o=t.querySelector("#mb-copy-status"),i={full:e.fullInfo,markdown:e.markdown,html:e.html,random:e.random,"random-ai":e.randomAI};t.querySelectorAll("[data-copy]").forEach(a=>{a.onclick=async()=>{const s=a.dataset.copy??"",l=i[s];if(l)try{await S(l),o.textContent="已复制，可以直接粘贴到你的友链页。"}catch{o.textContent="复制失败，请手动选中文本复制。"}}})}async function S(t){var i;if((i=navigator.clipboard)!=null&&i.writeText){await navigator.clipboard.writeText(t);return}const e=document.createElement("textarea");e.value=t,e.setAttribute("readonly","true"),e.style.position="fixed",e.style.left="-9999px",document.body.appendChild(e),e.select();const o=document.execCommand("copy");if(document.body.removeChild(e),!o)throw new Error("copy failed")}async function z(){if(!f())return;const e=v().querySelector(".mb-card");e.style.width="560px",e.style.maxHeight="calc(100vh - 32px)",e.style.overflowY="auto",e.innerHTML='<div style="text-align:center;color:#4a7a95;padding:24px 0">加载中...</div>',x();try{const o=await p("/api/submissions/mine");L(e,o.items)}catch(o){e.innerHTML=`<button class="mb-card-close">✕</button><div style="color:#ffa0a0;padding:20px 0;text-align:center">${r(o.message)}</div>`,e.querySelector(".mb-card-close").onclick=c}}function L(t,e){t.innerHTML=`
    <button class="mb-card-close" aria-label="关闭">✕</button>
    <h3>📮 我的提交 <span style="color:#4a7a95;font-size:0.82rem;font-weight:400;margin-left:6px">共 ${e.length} 个</span></h3>
    <p class="mb-card-sub">展示您提交过的所有博客及审核状态。</p>

    <button class="mb-btn" id="mb-new-sub" style="width:auto;padding:8px 16px;font-size:0.82rem;margin-bottom:12px">＋ 提交新博客</button>

    <div id="mb-sub-list" style="max-height:58vh;overflow-y:auto;margin:0 -4px;padding:0 4px"></div>
    <div class="mb-error" id="mb-sub-err"></div>
  `,t.querySelector(".mb-card-close").onclick=c,t.querySelector("#mb-new-sub").onclick=y;const o=t.querySelector("#mb-sub-err"),i=t.querySelector("#mb-sub-list");if(e.length===0){i.innerHTML='<div style="text-align:center;color:#4a7a95;padding:30px 0;font-size:0.85rem">还没有提交过任何博客</div>';return}for(const a of e)i.appendChild(C(a,o))}function M(t){switch(t){case"pending":return{text:"待审核",color:"#d8b45d"};case"approved":return{text:"已通过",color:"#6fd69b"};case"rejected":return{text:"未通过",color:"#d88a8a"};case"crawled":return{text:"已收录",color:"#7df8ff"}}}function C(t,e){const o=document.createElement("div");o.style.cssText="padding:12px;margin-bottom:10px;background:rgba(5,15,40,0.5);border:1px solid rgba(100,200,255,0.12);border-radius:10px";const i=M(t.status);o.innerHTML=`
    <div style="display:flex;align-items:flex-start;gap:10px">
      <div style="flex:1;min-width:0">
        <div style="display:flex;align-items:center;gap:8px;margin-bottom:4px">
          <span style="font-size:0.68rem;padding:2px 8px;border-radius:999px;background:${i.color}22;color:${i.color};border:1px solid ${i.color}44">${i.text}</span>
          <span style="font-size:0.72rem;color:#3a6a85">${r(new Date(t.submittedAt).toLocaleString("zh-CN"))}</span>
        </div>
        <div style="font-size:0.88rem;color:#c0e8ff;margin-bottom:3px;word-break:break-all">${r(t.title||"(未抓取到标题)")}</div>
        <div style="font-size:0.72rem;color:#3a8ab0;word-break:break-all">${r(t.url)}</div>
        ${t.rejectReason?`<div style="font-size:0.76rem;color:#d88a8a;margin-top:6px;padding:6px 10px;background:rgba(255,100,100,0.08);border-radius:6px">拒绝原因：${r(t.rejectReason)}</div>`:""}
      </div>
      ${t.status==="pending"?'<button class="mb-btn-link danger" data-act="cancel" style="padding:4px 8px;width:auto;margin:0;font-size:0.72rem">撤回</button>':""}
    </div>
  `;const a=o.querySelector('[data-act="cancel"]');return a&&(a.onclick=async()=>{if(confirm("确定撤回这个提交？"))try{await p(`/api/submissions/${encodeURIComponent(t.id)}`,{method:"DELETE"}),z()}catch(s){e.textContent=s.message,e.classList.add("show")}}),o}function r(t){return t.replace(/[&<>"']/g,e=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[e])}function g(t){return r(t).replace(/\s/g," ")}export{z as openMySubmissions,y as openSubmitBlog};
