//! A slot's game page: hints, Universal Tracker, PopTracker, log, files.

use super::*;

/// The per-game hint search. Expects `ITEMS` (this world's item names) defined
/// just above it, and `bifDropdown` from the page header (see `dropdown.js`
/// for how the list behaves). Enter submits the form.
pub(super) const HINT_JS: &str = r#"var q=document.getElementById('hintq');
var hlist=bifDropdown(q,document.getElementById('hintmatches'),function(){return ITEMS;},BIF_ITEM_LIST);
var hform=document.getElementById('hintform'),hstatus=document.getElementById('hintstatus');
var hback=hform.querySelector('input[name=back]').value,hbtn=hform.querySelector('button');
function refreshHints(){
 var el=document.getElementById('worldhints');
 [0,1200,2800].forEach(function(d){setTimeout(function(){
  if(el)fetch(hback.slice(1)+'/hints').then(function(r){return r.text()}).then(function(h){el.innerHTML=h;}).catch(function(){});
  // Refresh the search pool too: a name whose last copy was just hinted drops out.
  fetch(hback.slice(1)+'/items').then(function(r){return r.json()}).then(function(a){if(Array.isArray(a))ITEMS=a;}).catch(function(){});
 },d);});
}
hform.addEventListener('submit',function(e){
 e.preventDefault();
 var item=q.value.trim();if(!item)return;
 hlist.close();hbtn.disabled=true;hstatus.className='inlinemsg';hstatus.textContent='Hinting…';
 fetch('hint',{method:'POST',body:new URLSearchParams(new FormData(hform))}).then(function(r){return r.json()}).then(function(j){
  hbtn.disabled=false;
  hstatus.className='inlinemsg '+(j&&j.ok?'ok':'bad');
  hstatus.textContent=(j&&j.message)?j.message:(j&&j.ok?'Hinted.':'Hint failed.');
  if(j&&j.ok){q.value='';refreshHints();var pe=document.getElementById('hintpts');if(pe&&j.points!=null)pe.textContent=j.points;var ce=document.getElementById('bonushints');if(ce&&j.credits!=null)ce.textContent=j.credits;}
 }).catch(function(){hbtn.disabled=false;hstatus.className='inlinemsg bad';hstatus.textContent='Request failed.';});
});"#;

/// A common game's page: what its setup in the catalog says about joining it.
#[derive(Debug, Default)]
pub struct CommonView {
    pub key: String,
    pub name: String,
    /// Where players join its server (empty if nobody said).
    pub address: String,
    pub guide_html: String,
    /// The labels of its client downloads, in order.
    pub downloads: Vec<String>,
    pub ident: Option<CommonIdent>,
    /// False if its setup can't be read from the catalog.
    pub in_catalog: bool,
}

/// The name a common game's players register for its server's allowlist.
#[derive(Debug, Default)]
pub struct CommonIdent {
    pub label: String,
    pub hint: String,
    /// The viewer's, if they've registered one.
    pub current: Option<String>,
}

/// The per-game page: its players, hint with live item search,
/// and all hints in this world with a "prioritize" toggle.
#[allow(clippy::too_many_arguments)]
/// Game-page tab styling (Hints | Universal Tracker). Distinctive class names:
/// the shared stylesheet uses short utility names, so a generic class would
/// silently inherit unrelated rules (see the header note in this file).
pub(super) const GAME_STYLE: &str = r#"<style>
#tab-log>h2{text-align:center}
#tab-log .feed,#tab-log .pager{margin-inline:auto}
.gamelogbar{margin:0 0 10px;text-align:center}
.gamelogbar input[type=search]{width:100%;max-width:420px;box-sizing:border-box;background:var(--bg);color:var(--fg);border:1px solid var(--bd);border-radius:7px;padding:8px 11px;font-size:13px;font-family:var(--font)}
.gamelogbar input[type=search]:focus{outline:none;border-color:var(--accent)}
.utmeta{font-family:var(--mono);font-size:12px;color:var(--muted);margin:0 0 12px}
.utlist{display:flex;flex-direction:column;gap:4px;max-height:70vh;overflow:auto}
.utloc{font-size:13px;line-height:1.35;padding:6px 10px;background:var(--panel);border:1px solid var(--bd);border-radius:7px}
.utnote{font-size:12px;color:var(--muted);margin-top:14px;font-style:italic}
#tab-pt>h2{text-align:center}
.ptmap{width:100vw;max-width:100vw;margin-left:calc(50% - 50vw);margin-right:calc(50% - 50vw);padding-inline:clamp(16px,3vw,48px);box-sizing:border-box;text-align:center}
.ptmapwrap{position:relative;display:inline-block;max-width:100%;vertical-align:top;margin:0 7px 16px}
.ptgroup .ptgh{text-align:center}
/* Zoom/pan viewport: clips the transformed stage; touch-action:none so a drag
   pans instead of scrolling the page. */
.ptview{position:relative;overflow:hidden;width:fit-content;max-width:100%;border-radius:4px;touch-action:none}
.ptstage{position:relative;transform-origin:0 0}
.ptview img,.ptmapwrap img{display:block;max-width:100%;height:auto;user-select:none;-webkit-user-drag:none}
.ptmk{position:absolute;box-sizing:border-box;border-radius:2px}
.ptmk.d{transform:rotate(45deg)}
.ptform{margin:10px 0;display:flex;gap:8px;align-items:center;flex-wrap:wrap}
.ptform input[type=file]{color:var(--muted);font-size:12.5px;max-width:100%}
.ptmanage{margin-top:16px;text-align:center}.ptmanage>summary{cursor:pointer;color:var(--muted);font-size:13px}
.ptmanage .ptform{justify-content:center}
.ptnest{margin:4px 0}
.pttabs{display:inline-flex;flex-wrap:wrap;gap:2px;justify-content:center;border-bottom:1px solid var(--bd);margin:0 0 10px}
.pttab{background:none;color:var(--muted);border:0;border-bottom:2px solid transparent;border-radius:0;padding:5px 11px;font-family:var(--font);font-size:13px;font-weight:600;cursor:pointer;margin-bottom:-1px}
.pttab:hover{color:var(--fg)}.pttab.on{color:var(--accent);border-bottom-color:var(--accent)}
.ptgroup{margin:6px 0}.ptgh{font-family:var(--mono);font-size:11px;letter-spacing:.06em;text-transform:uppercase;color:var(--muted);margin:10px 0 4px}
.ptdock{display:flex;flex-direction:column;gap:14px}
.ptdockmid{display:flex;flex-direction:row;flex-wrap:wrap;gap:16px;justify-content:center;align-items:flex-start}
/* A bottom-docked strip (e.g. the item panels) gets a divider from the map
   above it. Shrink the strip to its content and center it so the divider line
   spans the item strip, not the full map width. */
.ptdockmid+.ptdock{border-top:1px solid var(--bd);padding-top:12px;align-self:center;width:fit-content;max-width:100%}
.ptarr{display:flex;gap:10px;flex-wrap:wrap;justify-content:center}
.ptgrid{display:inline-block;margin:4px 0}
.ptgrow{display:flex;gap:3px;justify-content:flex-start}
.ptcell{position:relative;display:inline-flex;width:34px;height:34px;align-items:center;justify-content:center}
.ptcell.empty{visibility:hidden}
.ptitem{width:100%;height:100%;object-fit:contain;image-rendering:pixelated}
.ptover{position:absolute;inset:0;pointer-events:none}
.ptin{position:absolute;border-radius:inherit}
.ptmk.trap,.ptmk.trap .ptin{clip-path:polygon(25% 0,75% 0,100% 100%,0 100%)}
.ptmk.tri,.ptmk.tri .ptin{clip-path:polygon(50% 0,100% 100%,0 100%)}
.ptmk.kite,.ptmk.kite .ptin{clip-path:polygon(50% 0,100% 100%,50% 70%,0 100%)}
.ptov{position:absolute;right:1px;bottom:0;font-family:var(--mono);font-size:10px;font-weight:700;color:#fff;text-shadow:0 0 2px #000,0 0 2px #000,0 0 2px #000;pointer-events:none}
</style>"#;

/// The game page's log tab (after [`TABS_JS`]).
pub(super) const GAME_TAB_JS: &str = r#"<script>
// The game's Log tab: a type-to-filter search + a feed whose page size is
// fitted to the viewport, with live refresh of the current page while open.
// Pager links (carrying the active search + rpp) are intercepted to swap in
// place. The first page is server-rendered; JS refits on show/resize.
(function(){var host=document.getElementById('gamelog');if(!host)return;
var slot=host.getAttribute('data-slot'),panel=document.getElementById('tab-log'),
    search=document.getElementById('gamelogq'),cur=null,tmr=null,rt=null,rpp=25;
function into(u){cur=u;fetch(u).then(function(r){return r.text();}).then(function(x){host.innerHTML=x;if(window.bifTick)bifTick();}).catch(function(){});}
function fit(){ // how many rows fit below the feed's top, within sane bounds
 var top=host.getBoundingClientRect().top,row=30,ev=host.querySelector('.ev');
 if(ev){var h=ev.getBoundingClientRect().height;if(h>0)row=h+6;}
 var n=Math.floor((window.innerHeight-top-64)/row);
 if(n<8)n=8;if(n>200)n=200;return n;}
function reload(page){rpp=fit();var q=search?search.value.trim():'';
 into('game/'+slot+'/log/fragment?page='+(page||1)+'&rpp='+rpp+(q?('&q='+encodeURIComponent(q)):''));}
host.addEventListener('click',function(e){var a=e.target.closest('a.pg');if(!a)return;e.preventDefault();into(a.getAttribute('href'));});
if(search){search.addEventListener('input',function(){clearTimeout(tmr);tmr=setTimeout(function(){reload(1);},200);});}
var bar=document.querySelector('.gtabs');
if(bar){bar.addEventListener('click',function(e){var b=e.target.closest('.gtab');if(b&&b.getAttribute('data-tab')==='tab-log')reload(1);});}
window.addEventListener('resize',function(){if(panel&&panel.hidden)return;clearTimeout(rt);rt=setTimeout(function(){reload(1);},250);});
setInterval(function(){if(panel&&!panel.hidden&&cur)into(cur);},12000);
})();
</script>"#;

/// Script that fills the PopTracker tab: fetch the computed tracker state and
/// draw each map image with accessibility-coloured markers at their pixel
/// coordinates (PopTracker palette: green available, yellow glitch, blue
/// viewable, red blocked, grey collected; mixed locations split diagonally),
/// polling while the tab is open.
pub(super) const PT_JS: &str = r#"<script>
(function(){
 var host=document.getElementById('ptmap'); if(!host) return;
 var slot=host.getAttribute('data-slot'); var st={}; var ver='';
 // PopTracker's exact default marker colors, keyed by accessibility level.
 var PT_COL={reachable:'#20ff20',glitch:'#ffff20',inspect:'#3040ff',none:'#cf1010',done:'#3f3f3f',partial:'#ff9f20'};
 // Rank so a split shows the best state top-left, worst bottom-right.
 var PT_RANK={reachable:4,glitch:3,inspect:2,none:1};
 function secLevel(s){ return s.checked?'done':(s.level||(s.accessible?'reachable':'none')); }
 function secColor(s){ return PT_COL[secLevel(s)]||PT_COL.none; }
 function secWord(s){ var l=secLevel(s);
  return l==='done'?'collected':l==='reachable'?'available':l==='glitch'?'available (glitch)':l==='inspect'?'viewable only':'blocked'; }
 // Background for a marker. Collected checks drop out (as in PopTracker); the
 // color reflects the checks that remain:
 //   all collected        -> grey
 //   one remaining state   -> that solid color
 //   two remaining states  -> diagonal split, bottom-left to top-right
 //   three or more states  -> solid orange "partial"
 // A hint's highlight on what's left of a marker: PopTracker's values, priority
 // first. -1 avoid, 1 no priority, 2 unspecified, 3 priority.
 var PT_HL={3:['#ffd700','priority'],'-1':['#ff4040','avoid'],1:['#909090','no priority'],2:['#40e0ff','hinted']};
 function markerHighlight(k){
  var best=0; (k.sections||[]).forEach(function(s){ if(!s.checked&&PT_HL[s.highlight]&&(best===0||s.highlight>best)) best=s.highlight; });
  return PT_HL[best];
 }
 function paintMarker(mk,k){
  // The marker is its border, in the map's own pixels so it scales with the map, and
  // the colour sits inside it. A hint shows as the border, made wide enough to see.
  var inner=mk.firstChild, hl=markerHighlight(k), inset=hl?Math.max(mk._inset,8):mk._inset;
  inner.style.background=markerBg(k); inner.style.inset=inset+'%';
  mk.style.background=hl?hl[0]:'rgba(0,0,0,.65)';
  mk.title=markerTitle(k); if(hl) mk.title+='\nHinted: '+hl[1];
 }
 function markerBg(k){
  var secs=(k.sections||[]);
  if(!secs.length){ return k.checked?PT_COL.done:(k.accessible?PT_COL.reachable:PT_COL.none); }
  var rem=secs.filter(function(s){return !s.checked;});
  if(!rem.length) return PT_COL.done;
  var cols=rem.map(secColor).filter(function(c,i,a){return a.indexOf(c)===i;});
  if(cols.length===1) return cols[0];
  if(cols.length>=3) return PT_COL.partial;
  cols.sort(function(a,b){ return (ptRankOf(b)-ptRankOf(a)); });
  // 135deg puts the dividing line bottom-left -> top-right; first color top-left.
  return 'linear-gradient(135deg,'+cols[0]+' 50%,'+cols[1]+' 50%)';
 }
 function ptRankOf(hex){ for(var k in PT_COL){ if(PT_COL[k]===hex&&PT_RANK[k]) return PT_RANK[k]; } return 0; }
 // Tooltip: the location name, then each check's name with its cleared/total
 // count and state (PopTracker shows every section a location holds).
 function markerTitle(k){
  var secs=(k.sections||[]);
  if(secs.length<=1){
   var s0=secs[0];
   var word=s0?secWord(s0):(k.checked?'collected':(k.accessible?'available':'blocked'));
   if(s0&&s0.item_count>1){ return k.location+' ('+word+')\n'+s0.cleared+'/'+s0.item_count+' checks'; }
   return k.location+' ('+word+')';
  }
  var lines=[k.location];
  secs.forEach(function(s){ lines.push('• '+(s.name||'(section)')+': '+s.cleared+'/'+s.item_count+' ('+secWord(s)+')'); });
  return lines.join('\n');
 }
 function assetUrl(f){ return 'game/'+slot+'/poptracker/asset?file='+encodeURIComponent(f)+(ver?('&v='+ver):''); }
 function drawMap(mapName){
  var m=(st.maps||[]).filter(function(x){return x.name===mapName;})[0];
  var wrap=document.createElement('div'); wrap.className='ptmapwrap';
  if(!m){ wrap.textContent='(map '+mapName+' missing)'; return wrap; }
  // A clipping viewport (.ptview) holds a transformable stage (.ptstage) with
  // the image + markers; scroll zooms toward the cursor, drag pans (like native
  // PopTracker). Markers are % of the stage, so they ride the transform.
  var view=document.createElement('div'); view.className='ptview';
  var stage=document.createElement('div'); stage.className='ptstage';
  view.appendChild(stage); wrap.appendChild(view);
  var img=document.createElement('img'); img.loading='lazy'; img.draggable=false;
  img.src=assetUrl(m.img); stage.appendChild(img);
  var sz=m.location_size||65; var spans=[];
  (st.markers||[]).filter(function(k){return k.map===mapName;}).forEach(function(k){
   var s=k.size||sz; var mk=document.createElement('span');
   var shape=k.shape||m.location_shape;
   mk.className='ptmk'+({diamond:' d',trapezoid:' trap',triangle:' tri',concave_kite:' kite'}[shape]||'');
   var turn={east:90,south:180,west:270}[k.orientation||m.location_orientation];
   if(turn&&(shape==='trapezoid'||shape==='triangle'||shape==='concave_kite')) mk.style.transform='rotate('+turn+'deg)';
   // As PACKS.md has it: the border goes outside the size, at this many map pixels.
   var edge=Number(k.border!=null?k.border:(m.location_border!=null?m.location_border:8));
   if(!isFinite(edge)||edge<0) edge=8;
   mk._edge=edge; mk._inset=edge/(s+2*edge)*100;
   mk.appendChild(document.createElement('span')).className='ptin';
   mk.dataset.key=k.location+'@'+k.map; mk.dataset.loc=k.location;
   paintMarker(mk,k);
   mk._k=k; mk._s=s; spans.push(mk); stage.appendChild(mk);
  });
  // Position markers as PERCENTAGES of the image's natural size, so they scale
  // with the responsive (max-width:100%) image on any screen width.
  function place(){ var W=img.naturalWidth||1, H=img.naturalHeight||1;
   spans.forEach(function(mk){ var k=mk._k, s=mk._s;
    var e=mk._edge, t=s+2*e;
    mk.style.left=((k.x-t/2)/W*100)+'%'; mk.style.top=((k.y-t/2)/H*100)+'%';
    mk.style.width=(t/W*100)+'%'; mk.style.height=(t/H*100)+'%';
   });
  }
  if(img.complete && img.naturalWidth) place(); else img.addEventListener('load',place);
  attachZoomPan(view,stage);
  return wrap;
 }
 // Scroll-to-zoom + drag-to-pan (mouse) and pinch/drag (touch) on one map view.
 function attachZoomPan(view,stage){
  var scale=1,tx=0,ty=0,MAX=8;
  function apply(){ stage.style.transform='translate('+tx+'px,'+ty+'px) scale('+scale+')';
   view.style.cursor=scale>1?'grab':''; view.classList.toggle('zoomed',scale>1); }
  function clamp(){ var r=view.getBoundingClientRect(), W=r.width, H=r.height;
   var minx=W*(1-scale), miny=H*(1-scale);
   tx=Math.min(0,Math.max(minx,tx)); ty=Math.min(0,Math.max(miny,ty));
   if(scale<=1){ tx=0; ty=0; } }
  function zoomAt(px,py,factor){ var ns=Math.min(MAX,Math.max(1,scale*factor));
   if(ns===scale) return; var cx=(px-tx)/scale, cy=(py-ty)/scale;
   scale=ns; tx=px-cx*scale; ty=py-cy*scale; clamp(); apply(); }
  view.addEventListener('wheel',function(e){ e.preventDefault(); var r=view.getBoundingClientRect();
   zoomAt(e.clientX-r.left, e.clientY-r.top, e.deltaY<0?1.2:1/1.2); }, {passive:false});
  view.addEventListener('dblclick',function(e){ e.preventDefault(); scale=1; tx=0; ty=0; apply(); });
  var pts={}, last=null, pinch=0;
  view.addEventListener('pointerdown',function(e){ pts[e.pointerId]={x:e.clientX,y:e.clientY};
   try{ view.setPointerCapture(e.pointerId); }catch(_){}
   if(Object.keys(pts).length===1){ last={x:e.clientX,y:e.clientY}; if(scale>1) view.style.cursor='grabbing'; } });
  view.addEventListener('pointermove',function(e){ if(!pts[e.pointerId]) return;
   pts[e.pointerId]={x:e.clientX,y:e.clientY}; var ids=Object.keys(pts);
   if(ids.length>=2){ var a=pts[ids[0]], b=pts[ids[1]], d=Math.hypot(a.x-b.x,a.y-b.y);
    var r=view.getBoundingClientRect(); if(pinch) zoomAt((a.x+b.x)/2-r.left,(a.y+b.y)/2-r.top,d/pinch);
    pinch=d; last=null; return; }
   if(last){ if(scale>1){ tx+=e.clientX-last.x; ty+=e.clientY-last.y; clamp(); apply(); }
    last={x:e.clientX,y:e.clientY}; } });
  function up(e){ delete pts[e.pointerId]; try{ view.releasePointerCapture(e.pointerId); }catch(_){}
   if(Object.keys(pts).length<2) pinch=0;
   if(!Object.keys(pts).length){ last=null; view.style.cursor=scale>1?'grab':''; } }
  view.addEventListener('pointerup',up); view.addEventListener('pointercancel',up);
  apply();
 }
 var itemsByCode={};
 function buildIndex(){ itemsByCode={}; (st.items||[]).forEach(function(it){ (it.codes||[]).forEach(function(c){ if(c) itemsByCode[(''+c).toLowerCase()]=it; }); }); }
 // An item's image filters, as PopTracker defines them. The engine has reduced them
 // to these kinds and numbers.
 function cssFilter(fs){
  return (fs||[]).map(function(f){
   if(f.kind==='grey') return 'saturate(0) brightness(.67)';
   if(f.kind==='saturation') return 'saturate('+Number(f.value)+')';
   if(f.kind==='brightness') return 'brightness('+Number(f.value)+')';
   return '';
  }).join(' ');
 }
 function paintItem(im,it){
  // Most polls change nothing, and rebuilt images can flicker.
  var drawn=JSON.stringify([it.img,it.filters]);
  if(im._drawn===drawn) return;
  im._drawn=drawn;
  if(it.img) im.src=assetUrl(it.img);
  var cell=im.parentNode, fs=it.filters||[];
  // In order, as PopTracker applies them: a filter after an overlay affects it too.
  im.style.filter=cssFilter(fs.filter(function(f){return f.kind!=='overlay';}))||'';
  cell.querySelectorAll('.ptover').forEach(function(o){o.remove();});
  var last=im;
  fs.forEach(function(f,i){ if(f.kind!=='overlay') return;
   var o=document.createElement('img'); o.className='ptitem ptover'; o.alt=''; o.src=assetUrl(f.img);
   o.style.filter=cssFilter((f.filters||[]).concat(fs.slice(i+1).filter(function(g){return g.kind!=='overlay';})))||'';
   last.insertAdjacentElement('afterend',o); last=o; });
 }
 function itemCell(code){
  var cell=document.createElement('span'); cell.className='ptcell';
  if(!code){ cell.classList.add('empty'); return cell; }
  var it=itemsByCode[(''+code).toLowerCase()];
  if(it&&it.img){
   var im=document.createElement('img'); im.className='ptitem'; im.dataset.code=code; im.alt=it.name||code;
   cell.title=it.name||code; cell.appendChild(im); paintItem(im,it);
   if(it.overlay){ var o=document.createElement('span'); o.className='ptov'; o.textContent=it.overlay; cell.appendChild(o); }
  }
  return cell;
 }
 function appendContent(parent,c){ if(c==null) return; if(Array.isArray(c)){ c.forEach(function(x){ parent.appendChild(render(x)); }); } else { parent.appendChild(render(c)); } }
 function render(n){
  if(n==null) return document.createComment('');
  if(Array.isArray(n)){ var f=document.createElement('div'); n.forEach(function(c){f.appendChild(render(c));}); return f; }
  if(typeof n!=='object') return document.createComment('');
  var t=n.type;
  if(t==='map'){ var d=document.createElement('div'); (n.maps||[]).forEach(function(mn){d.appendChild(drawMap(mn));}); return d; }
  if(t==='tabbed'){
   var box=document.createElement('div'); box.className='ptnest';
   var bar=document.createElement('div'); bar.className='pttabs';
   var body=document.createElement('div'); box.appendChild(bar); box.appendChild(body);
   (n.tabs||[]).forEach(function(tab,i){
    var btn=document.createElement('button'); btn.type='button'; btn.className='pttab'+(i===0?' on':''); btn.textContent=tab.title||('Tab '+(i+1));
    var panel=render(tab.content); panel.hidden=(i!==0);
    btn.addEventListener('click',function(){
     bar.querySelectorAll('.pttab').forEach(function(x){x.classList.remove('on');}); btn.classList.add('on');
     Array.prototype.forEach.call(body.children,function(p){p.hidden=true;}); panel.hidden=false;
    });
    bar.appendChild(btn); body.appendChild(panel);
   });
   return box;
  }
  if(t==='itemgrid'){ var g=document.createElement('div'); g.className='ptgrid';
   (n.rows||[]).forEach(function(row){ var r=document.createElement('div'); r.className='ptgrow'; (row||[]).forEach(function(code){ r.appendChild(itemCell(code)); }); g.appendChild(r); });
   return g;
  }
  if(t==='item'){ var ic=document.createElement('div'); ic.className='ptgrow'; ic.appendChild(itemCell(n.item||n.code||'')); return ic; }
  if(t==='array'){ var a=document.createElement('div'); a.className='ptarr';
   a.style.flexDirection=(n.orientation==='horizontal'?'row':'column'); a.style.alignItems=(n.orientation==='horizontal'?'flex-start':'center');
   appendContent(a,n.content); return a;
  }
  if(t==='group'){ var gr=document.createElement('div'); gr.className='ptgroup';
   if(n.header){ var h=document.createElement('div'); h.className='ptgh'; h.textContent=n.header; gr.appendChild(h); }
   appendContent(gr,n.content); return gr;
  }
  // PopTracker dock: children carry a `dock` edge (top/bottom/left/right) and
  // undocked children fill the center. We honor it so docked panels land on
  // their edge — e.g. Stardew's item grids are dock:"bottom", so they belong
  // under the map, not above it (array order alone would float them to the top).
  if(t==='dock'){
   var kids=[]; (function col(c){ if(c==null) return; if(Array.isArray(c)) c.forEach(col); else kids.push(c); })(n.content);
   var b={top:[],bottom:[],left:[],right:[],center:[]};
   kids.forEach(function(k){ (b[(k&&k.dock)]||b.center).push(k); });
   var box=document.createElement('div'); box.className='ptdock';
   b.top.forEach(function(k){ box.appendChild(render(k)); });
   if(b.left.length||b.center.length||b.right.length){
    var mid=document.createElement('div'); mid.className='ptdockmid';
    b.left.concat(b.center,b.right).forEach(function(k){ mid.appendChild(render(k)); });
    box.appendChild(mid);
   }
   b.bottom.forEach(function(k){ box.appendChild(render(k)); });
   return box;
  }
  if(n.content!==undefined){ var cc=document.createElement('div'); appendContent(cc,n.content); return cc; }
  return document.createComment(t||'');
 }
 function update(){
  var mm={}; (st.markers||[]).forEach(function(k){mm[k.location+'@'+k.map]=k;});
  host.querySelectorAll('.ptmk').forEach(function(mk){ var k=mm[mk.dataset.key]; if(k){ paintMarker(mk,k); } });
  host.querySelectorAll('.ptitem:not(.ptover)').forEach(function(im){ var it=itemsByCode[(''+im.dataset.code).toLowerCase()]; if(!it) return;
   paintItem(im,it);
   var ov=im.parentNode.querySelector('.ptov');
   if(it.overlay){ if(!ov){ ov=document.createElement('span'); ov.className='ptov'; im.parentNode.appendChild(ov); } ov.textContent=it.overlay; }
   else if(ov){ ov.remove(); }
  });
 }
 var retry=0;
 function draw(d){
  if(!d||!d.pack||d.error){ var old=document.getElementById('ptnote'); if(old) old.remove(); }
  if(!d||!d.pack){ host.textContent='No pack.'; return; }
  if(d.busy){ if(!retry) retry=setTimeout(function(){ retry=0; if(panel&&!panel.hidden) load(); },2000); return; }
  if(d.error){ host.textContent='Tracker error: '+d.error; delete host.dataset.built; return; }
  ver=(d.version!=null?(''+d.version):''); st=d.state||{}; buildIndex();
  var note=document.getElementById('ptnote');
  if(st.pack_error_count){ if(!note){ note=document.createElement('p'); note.id='ptnote'; note.className='muted'; host.parentNode.insertBefore(note,host); }
   note.textContent='This pack reported '+st.pack_error_count+' error(s), so some of its tracking may be missing.'; note.title=(st.pack_errors||[]).join('\n'); }
  else if(note){ note.remove(); }
  if(host.dataset.built){ update(); return; }
  host.innerHTML='';
  if(st.layout){ host.appendChild(render(st.layout)); }
  else if(st.maps&&st.maps.length){ st.maps.forEach(function(m){ host.appendChild(drawMap(m.name)); }); }
  else { host.textContent='This pack has no maps to show.'; return; }
  host.dataset.built='1';
 }
 function load(){ fetch('game/'+slot+'/poptracker/state').then(function(r){return r.json();}).then(draw).catch(function(){}); }
 var panel=document.getElementById('tab-pt'); var bar=document.querySelector('.gtabs');
 if(bar){ bar.addEventListener('click',function(e){var b=e.target.closest('.gtab'); if(b&&b.getAttribute('data-tab')==='tab-pt') load();}); }
 load();
 setInterval(function(){ if(panel&&!panel.hidden) load(); },12000);
})();
</script>"#;

/// The PopTracker tab panel: the map host (filled by `PT_JS`) + upload/manage
/// controls for a claimant or admin, or a prompt/placeholder otherwise.
pub(super) fn poptracker_panel(game_id: Id, has_pack: bool, can_manage: bool) -> String {
    let mut s = String::from(r#"<div class="gtabpanel" id="tab-pt" hidden><h2>PopTracker</h2>"#);
    if has_pack {
        s.push_str(&format!(
            r#"<div class="ptmap" id="ptmap" data-slot="{game_id}"><p class="empty">Loading tracker…</p></div>"#
        ));
        if can_manage {
            s.push_str(&format!(
                r#"<details class="ptmanage"><summary>Manage pack</summary>
<form method="post" action="game/{game_id}/poptracker" enctype="multipart/form-data" class="ptform">
<input type="file" name="pack" accept=".zip,application/zip" required><button type="submit">Replace pack</button></form>
<form method="post" action="game/{game_id}/poptracker/remove" onsubmit="return confirm('Remove this pack?')"><button type="submit" class="ghost">Remove pack</button></form></details>"#
            ));
        }
        s.push_str(PT_JS);
    } else if can_manage {
        s.push_str(&format!(
            r#"<p class="empty">Upload a <a href="https://github.com/black-sliver/PopTracker" target="_blank" rel="noopener">PopTracker</a> pack (.zip) for this game to get its map here, auto-tracked from the server — no setup for anyone viewing. The pack must support Archipelago auto-tracking.</p>
<form method="post" action="game/{game_id}/poptracker" enctype="multipart/form-data" class="ptform">
<input type="file" name="pack" accept=".zip,application/zip" required><button type="submit">Upload pack</button></form>"#
        ));
    } else {
        s.push_str(
            r#"<p class="empty">No PopTracker pack has been uploaded for this game yet.</p>"#,
        );
    }
    s.push_str("</div>");
    s
}

/// Game Log tab page size: the server default (before the client fits it to the
/// viewport) and the clamp bounds the fitted value must stay within.
pub const GAME_LOG_RPP: i64 = 25;
pub const GAME_LOG_RPP_MIN: i64 = 8;
pub const GAME_LOG_RPP_MAX: i64 = 200;

/// Pages for a given total at `rpp` (at least 1).
pub fn game_log_pages(total: i64, rpp: i64) -> i64 {
    ((total + rpp - 1) / rpp.max(1)).max(1)
}

/// Inner HTML of a game's Log feed (the `#gamelog` contents): one page of rows
/// plus a numbered pager. Reused by the tab and its live-refresh fragment; the
/// pager links carry the active search `qs` + `rpp` and are intercepted
/// client-side so navigation swaps in place.
pub fn game_log_feed(
    game_id: Id,
    rows: &[crate::db::ActivityRow],
    page: i64,
    pages: i64,
    rpp: i64,
    qs: &str,
) -> String {
    let mut s = String::new();
    if rows.is_empty() {
        s.push_str(r#"<p class="empty">Nothing matches.</p>"#);
        return s;
    }
    render_feed(&mut s, rows, None);
    let base = format!("game/{game_id}/log/fragment");
    s.push_str(&activity_pager(&base, qs, page, pages, rpp));
    s
}

/// The game page's "Log" tab: this slot's own activity (items it sent or
/// received, its goals/releases/collects), newest first, with a type-to-filter
/// search box and a viewport-fitted paginated feed.
pub(super) fn log_panel(game_id: Id, rows: &[crate::db::ActivityRow], total: i64) -> String {
    let mut s = String::from(
        r#"<div class="gtabpanel" id="tab-log" hidden><h2>This game's activity</h2>
<div class="gamelogbar"><input type="search" id="gamelogq" placeholder="Search this game's activity…" autocomplete="off"></div>"#,
    );
    s.push_str(&format!(
        r#"<div id="gamelog" class="gamelog" data-slot="{game_id}">"#
    ));
    s.push_str(&game_log_feed(
        game_id,
        rows,
        1,
        game_log_pages(total, GAME_LOG_RPP),
        GAME_LOG_RPP,
        "",
    ));
    s.push_str("</div></div>");
    s
}

/// The Universal Tracker tab panel: its contents (refreshed by `UT_JS` from
/// `game/<id>/inlogic`) and the script that keeps them current.
pub(super) fn inlogic_panel(game_id: Id, inlogic: Option<&crate::db::InLogic>) -> String {
    format!(
        r#"<div class="gtabpanel" id="tab-ut" hidden><h2>Available now</h2><div id="utbody" data-slot="{game_id}">{}</div></div>{UT_JS}"#,
        inlogic_body(inlogic)
    )
}

/// The header's figures for a slot.
pub fn game_stats(live: &crate::ap::ApLive) -> String {
    format!(
        "{} locations · {} left · sent {} · received {}",
        live.world_size, live.still_to_find, live.sent, live.received
    )
}

/// Keeps the header's figures and the hints list up to date while the page is
/// open: every 12 seconds, and when the page comes back into view. The list is
/// left alone while its tab is hidden or someone's using it.
const LIVE_JS: &str = r#"<script>
(function(){
 var stats=document.getElementById('gamestats'); if(!stats) return;
 var slot=stats.getAttribute('data-slot'), hints=document.getElementById('worldhints'), panel=document.getElementById('tab-hints');
 function busy(){ return hints && hints.contains(document.activeElement); }
 function load(){
  if(document.hidden) return;
  var want=hints&&!(panel&&panel.hidden)&&!busy();
  fetch('game/'+slot+'/live'+(want?'':'?hints=0')).then(function(r){return r.json();}).then(function(j){
   if(!j||typeof j.stats!=='string') return;
   stats.textContent=j.stats;
   if(want&&typeof j.hints==='string'&&!busy()) hints.innerHTML=j.hints;
   if(window.bifTick) bifTick();
  }).catch(function(){});
 }
 setInterval(load,12000);
 document.addEventListener('visibilitychange',function(){ if(!document.hidden) load(); });
})();
</script>"#;

/// While the Universal Tracker tab is open, fetch its latest result every 12s
/// (Bifrost recomputes it whenever the slot's items or checks change), and at
/// once when the tab is opened. The header's "available" count follows.
const UT_JS: &str = r#"<script>
(function(){
 var host=document.getElementById('utbody'); if(!host) return;
 var slot=host.getAttribute('data-slot'), panel=document.getElementById('tab-ut');
 function load(){ fetch('game/'+slot+'/inlogic').then(function(r){return r.json();}).then(function(j){
  if(!j||typeof j.body!=='string') return;
  host.innerHTML=j.body;
  var a=document.getElementById('utavail'); if(a) a.textContent=j.avail;
  if(window.bifTick) bifTick();
 }).catch(function(){}); }
 var bar=document.querySelector('.gtabs');
 if(bar){ bar.addEventListener('click',function(e){var b=e.target.closest('.gtab'); if(b&&b.getAttribute('data-tab')==='tab-ut') load();}); }
 setInterval(function(){ if(panel&&!panel.hidden) load(); },12000);
})();
</script>"#;

/// The header's "· N available", from the slot's Universal Tracker result.
pub fn inlogic_avail(inlogic: Option<&crate::db::InLogic>) -> String {
    inlogic
        .filter(|il| il.err.is_none())
        .and_then(|il| serde_json::from_str::<Vec<String>>(&il.locations).ok())
        .map(|v| format!(" · {} available", v.len()))
        .unwrap_or_default()
}

/// The Universal Tracker tab's contents, from the slot's cached in-logic result.
pub fn inlogic_body(inlogic: Option<&crate::db::InLogic>) -> String {
    let mut s = String::new();
    match inlogic {
        None => s.push_str(
            r#"<p class="empty">Universal Tracker hasn't mapped this slot yet — it recomputes a few seconds after the slot's items or checks change. Check back shortly.</p>"#,
        ),
        Some(il) => {
            let locs: Vec<String> = serde_json::from_str(&il.locations).unwrap_or_default();
            if locs.is_empty() && il.err.is_some() {
                let msg = match il.err.as_deref() {
                    Some("disabled") => "Universal Tracker is turned off for this game by its developers, so live availability tracking isn't available here.",
                    _ => "Couldn't compute available locations for this slot right now — it'll retry automatically.",
                };
                s.push_str(&format!(r#"<p class="empty">{msg}</p>"#));
            } else {
                let n = locs.len();
                s.push_str(&format!(
                    r#"<p class="utmeta"><b>{n}</b> location{} available right now · as of <span class="ago" data-ts="{}"></span></p>"#,
                    if n == 1 { "" } else { "s" },
                    il.ts,
                ));
                if locs.is_empty() {
                    s.push_str(r#"<p class="empty">Nothing new is available right now.</p>"#);
                } else {
                    s.push_str(r#"<div class="utlist">"#);
                    for l in &locs {
                        s.push_str(&format!(r#"<div class="utloc">{}</div>"#, esc(l)));
                    }
                    s.push_str("</div>");
                }
                s.push_str(r#"<p class="utnote">Computed by Universal Tracker from this slot's current items. Randomized options (entrance randomizer / weighted choices) can make it differ slightly from the server.</p>"#);
            }
        }
    }
    s
}

/// A common game's card above the tabs: what it is, where its server is, and
/// the player's name for its allowlist, or the form to register one.
fn common_card(c: &CommonView, can_register: bool, back: &str) -> String {
    let mut out = format!(
        r#"<h2>Common game</h2><p class="empty"><b>{}</b> is shared by everyone this season: drop in when you're stuck, taking a break or done with your own game. Its checks send items to the rest of the group.</p>"#,
        esc(&c.name)
    );
    if !c.address.is_empty() {
        out.push_str(&format!(
            r#"<p class="statline">Server: <b>{}</b></p>"#,
            esc(&c.address)
        ));
    }
    let Some(id) = c.ident.as_ref().filter(|_| c.in_catalog) else {
        return out;
    };
    if !can_register {
        out.push_str(&format!(
            r#"<p class="empty">The server only lets in this season's players. Play a game this season, then come back here to register your {}.</p>"#,
            esc(&id.label)
        ));
        return out;
    }
    let form = |cur: &str, btn: &str| {
        format!(
            r#"<form method="post" action="common/{key}/ident" class="hintform">
<input type="hidden" name="back" value="{back}">
<input name="name" value="{cur}" maxlength="64" autocomplete="off" aria-label="{label}" required>
<button type="submit">{btn}</button></form>"#,
            key = esc(&c.key),
            back = esc(back),
            cur = esc(cur),
            label = esc(&id.label),
        )
    };
    match id.current.as_deref() {
        Some(cur) => out.push_str(&format!(
            r#"<p class="statline">Your {label}: <b>{}</b>, on the allowlist.</p><details class="identedit"><summary>Change it</summary>{}</details>"#,
            esc(cur),
            form(cur, "Update"),
            label = esc(&id.label),
        )),
        None => {
            let hint = if id.hint.is_empty() {
                String::new()
            } else {
                format!(" ({})", esc(&id.hint))
            };
            out.push_str(&format!(
                r#"<p class="empty">The server only lets in this season's players. Register the exact {} you'll join with{hint}: the admins put it on the server's allowlist.</p>{}"#,
                esc(&id.label),
                form("", "Register"),
            ));
        }
    }
    out
}

/// A common game's How to join tab: its guide and client downloads.
fn common_guide(c: &CommonView) -> String {
    if !c.in_catalog {
        return r#"<p class="empty">How to join isn't available right now: ask an admin.</p>"#
            .to_string();
    }
    let mut out = String::new();
    if !c.guide_html.is_empty() {
        out.push_str(r#"<div class="guide">"#);
        out.push_str(&c.guide_html);
        out.push_str("</div>");
    }
    if !c.downloads.is_empty() {
        out.push_str(r#"<h2>Downloads</h2><ul class="downloads">"#);
        for (n, label) in c.downloads.iter().enumerate() {
            out.push_str(&format!(
                r#"<li><a href="common/{}/file/{n}">{}</a></li>"#,
                esc(&c.key),
                esc(label)
            ));
        }
        out.push_str("</ul>");
    }
    if out.is_empty() {
        out.push_str(r#"<p class="empty">This common game has no join guide. Ask an admin.</p>"#);
    }
    out
}

/// Whether a common game's page opens on How to join: for a player who can't
/// hint there yet (no game this season), and until the player has registered
/// the name its server lets in by.
fn join_first(c: &CommonView, can_register: bool) -> bool {
    !can_register || c.ident.as_ref().is_some_and(|id| id.current.is_none())
}

#[allow(clippy::too_many_arguments)]
pub fn game_page(
    ap_slot: i64,
    game_id: Id,
    game: &str,
    snap: &BoardSnapshot,
    slots: &[SlotRow],
    prios: &std::collections::HashSet<(i64, i64, i64)>,
    item_names: &[String],
    me: &str,
    is_admin: bool,
    cost: i64,
    points: Option<i64>,
    credits: i64,
    minesweeper_on: bool,
    can_hint_common: bool,
    common: Option<&CommonView>,
    files: &[crate::generate::OutputFile],
    inlogic: Option<&crate::db::InLogic>,
    log: &[crate::db::ActivityRow],
    log_total: i64,
    pt_has_pack: bool,
    pt_can_manage: bool,
    invite: &str,
) -> Html<String> {
    let live = snap.ap.get(&ap_slot).cloned().unwrap_or_default();
    let back = format!("/game/{game_id}");
    let here: Vec<&SlotRow> = slots.iter().filter(|r| r.ap_slot == ap_slot).collect();
    let mut out = page_head(Some(me));
    out.push_str(GAME_STYLE);
    out.push_str(TABS_CSS);
    // The UT and Log tab panels are the same for the common and regular slot
    // branches; build them once.
    let ut_html = inlogic_panel(game_id, inlogic);
    let log_html = log_panel(game_id, log, log_total);
    // "available now" count from the slot's Universal Tracker result (if any).
    let avail = inlogic_avail(inlogic);
    out.push_str(&format!(
        r#"<h1>{}</h1><div class="crumb"><a href="board">← board</a>{}</div>
<div class="meta"><span id="gamestats" data-slot="{game_id}">{}</span><span id="utavail">{avail}</span></div>"#,
        // A common game goes by its setup's name, as on the board.
        esc(common.map_or(game, |c| c.name.as_str())),
        if is_admin { " · admin" } else { "" },
        game_stats(&live),
    ));

    // Admin: mark this slot finished (registers its goal → collects/releases its
    // items per server settings). Hidden once goaled; /release stays separate.
    if is_admin {
        if live.status == 30 {
            out.push_str(r#"<div class="adminfin"><span class="finnote">✓ Marked finished (goaled).</span></div>"#);
        } else {
            out.push_str(&format!(
                r#"<form class="adminfin" method="post" action="game/{game_id}/finish" onsubmit="return confirm('Mark this game as finished? It registers their goal (announces it) and collects/releases their items per the server settings. This can\'t be undone from here.')">
<input type="hidden" name="back" value="{back}">
<button type="submit" class="finish">Mark finished</button></form>"#
            ));
        }
    }

    // A common game: shared by everyone, with no claim. Show how to join its
    // server instead of the claim and hint controls.
    if let Some(c) = common {
        out.push_str(&common_card(c, can_hint_common, &back));

        // Tabs: everything hint-related goes in the Hints panel, how to join
        // in its own, which comes first until the player is on the allowlist.
        let join = join_first(c, can_hint_common);
        let (hints_on, join_on) = if join { ("", " on") } else { (" on", "") };
        out.push_str(&format!(
            r#"<div class="gtabs"><button type="button" class="gtab{hints_on}" data-tab="tab-hints">Hints</button><button type="button" class="gtab{join_on}" data-tab="tab-join">How to join</button><button type="button" class="gtab" data-tab="tab-ut">Universal Tracker</button><button type="button" class="gtab" data-tab="tab-pt">PopTracker</button><button type="button" class="gtab" data-tab="tab-log">Log</button></div><div class="gtabpanel" id="tab-hints"{}>"#,
            if join { " hidden" } else { "" }
        ));

        // Communal hinting: only players with a claimed game this season may spend
        // the common game's OWN (shared) AP hint points — no personal/bonus points.
        if can_hint_common {
            if let Some(sid) = here.first().map(|r| r.id) {
                let items_json = serde_json::to_string(item_names).unwrap_or_else(|_| "[]".into());
                let have = match points {
                    Some(p) => format!(
                        r#"The common game's shared hint points: <b id="hintpts">{p}</b>. "#
                    ),
                    None => String::new(),
                };
                out.push_str(&format!(
                    r#"<h2>Send a hint</h2>
<p class="empty">{have}Each hint in this world costs <b>{cost}</b> points. Any player with a game this season can spend them — they're the common game's own, earned as the group checks its locations. Search the pool and hint one.</p>
<form method="post" action="hint" class="hintform" id="hintform">
<input type="hidden" name="slot_id" value="{sid}"><input type="hidden" name="back" value="{back}">
<input id="hintq" name="item" placeholder="search the common game's items…" autocomplete="off" required>
<button type="submit">Hint</button></form>
<div id="hintmatches" class="matches"></div>
<div id="hintstatus" class="inlinemsg"></div>"#
                ));
                out.push_str("<script>\nvar ITEMS=");
                out.push_str(&super::script_json(&items_json));
                out.push_str(";\n");
                out.push_str(HINT_JS);
                out.push_str("\n</script>");
            }
        } else {
            out.push_str(
                r#"<h2>Send a hint</h2><p class="empty">Hinting the common game is open to anyone with a game this season — its shared hint points are reserved for players with a slot of their own. Sign up with a game, or join one with an invite link, to take part.</p>"#,
            );
        }

        // Where the common game's items are. Admins get the prioritize toggles.
        out.push_str(
            r#"<h2>Hints for this game's items</h2><p class="empty" style="margin-top:-4px">Where the common game's items are — in its own or others' worlds.</p><div id="worldhints">"#,
        );
        out.push_str(&hint_rows(
            &live.hint_views,
            &live.found_items,
            prios,
            &back,
            is_admin,
        ));
        out.push_str("</div>"); // close #worldhints
        out.push_str("</div>"); // close #tab-hints
        out.push_str(&format!(
            r#"<div class="gtabpanel" id="tab-join"{}>{}</div>"#,
            if join { "" } else { " hidden" },
            common_guide(c)
        ));
        out.push_str(&ut_html);
        out.push_str(&poptracker_panel(game_id, pt_has_pack, pt_can_manage));
        out.push_str(&log_html);
        out.push_str(TABS_JS);
        out.push_str(GAME_TAB_JS);
        out.push_str(LIVE_JS);
        out.push_str("</main></body></html>");
        return Html(out);
    }

    // Players
    out.push_str(r#"<h2>Players</h2><div class="grid players">"#);
    for r in &here {
        let mine = r.claimed_by.as_deref() == Some(me);
        let (by, action) = match &r.claimed_by {
            Some(_) if mine => (
                r#"<span class="by mine">you</span>"#.to_string(),
                String::new(),
            ),
            Some(who) => (
                format!(r#"<span class="by">{}</span>"#, esc(who)),
                String::new(),
            ),
            None => (
                r#"<span class="by free">unclaimed</span>"#.to_string(),
                if is_admin {
                    assign_form(r.id, &back)
                } else {
                    String::new()
                },
            ),
        };
        out.push_str(&format!(
            r#"<div class="slot idle"><div class="prow"><span class="who">{}</span><span>{by} {action}</span></div></div>"#,
            esc(&r.name)
        ));
    }
    out.push_str("</div>");
    out.push_str(invite);

    // Generated files for this slot — shown only to a claimant of it (or admin);
    // the caller passes an empty list to everyone else. Downloaded through the
    // claimant-gated /game/{game}/output route (not the admin one).
    if !files.is_empty() {
        out.push_str(r#"<h2>Your game files</h2>"#);
        out.push_str(r#"<p class="empty">The generator produced these for your slot — download and use them with your client to play this seed.</p>"#);
        out.push_str(r#"<div class="grid">"#);
        for f in files {
            out.push_str(&format!(
                r#"<div class="slot idle"><div class="prow"><span class="who"><code>.{}</code> file · {}</span><a class="ghost" href="game/{game_id}/output?file={}">download</a></div></div>"#,
                esc(&f.ext),
                human_size(f.size),
                urlq(&f.member),
            ));
        }
        out.push_str("</div>");
    }

    // Tabs: the hint form + per-item hint list live in the Hints panel; the
    // Universal Tracker panel (built above) sits beside it. Players/files above
    // stay outside the tabs.
    out.push_str(tabs_open());

    // Hinting — shown only to a player who claims a seat on this slot. Admins do
    // not get it here; they hint via the server console instead.
    let hint_slot = here
        .iter()
        .find(|r| r.claimed_by.as_deref() == Some(me))
        .map(|r| r.id);
    if let Some(sid) = hint_slot {
        let items_json = serde_json::to_string(item_names).unwrap_or_else(|_| "[]".into());
        let have = match points {
            Some(p) => format!(r#"Your hint points: <b id="hintpts">{p}</b>. "#),
            None => String::new(),
        };
        let bonus = if credits > 0 {
            format!(
                r#" Bonus hints: <b id="bonushints">{credits}</b>, spent automatically when you're out of points."#
            )
        } else {
            String::new()
        };
        let mines = if minesweeper_on {
            format!(
                r#"<p class="empty"><a href="minesweeper?back={back}">Play minesweeper to earn bonus hints →</a></p>"#
            )
        } else {
            String::new()
        };
        out.push_str(&format!(
            r#"<h2>Send a hint</h2>
<p class="empty">{have}Each hint in this world costs <b>{cost}</b> points.{bonus} Search your pool and hint one.</p>
{mines}
<form method="post" action="hint" class="hintform" id="hintform">
<input type="hidden" name="slot_id" value="{sid}"><input type="hidden" name="back" value="{back}">
<input id="hintq" name="item" placeholder="search your items…" autocomplete="off" required>
<button type="submit">Hint</button></form>
<div id="hintmatches" class="matches"></div>
<div id="hintstatus" class="inlinemsg"></div>"#
        ));
        out.push_str("<script>\nvar ITEMS=");
        out.push_str(&super::script_json(&items_json));
        out.push_str(";\n");
        out.push_str(HINT_JS);
        out.push_str("\n</script>");
    }

    // Hints for this game's items — where each of this slot's items is located.
    // Only a claimant of this game (or an admin) gets the prioritize toggles.
    // Wrapped in #worldhints so a successful hint can live-refresh it.
    let can_prioritize = hint_slot.is_some() || is_admin;
    out.push_str(
        r#"<h2>Hints for this game's items</h2><p class="empty" style="margin-top:-4px">Where this slot's items are — in its own or others' worlds.</p><div id="worldhints">"#,
    );
    out.push_str(&hint_rows(
        &live.hint_views,
        &live.found_items,
        prios,
        &back,
        can_prioritize,
    ));
    out.push_str("</div>"); // close #worldhints
    out.push_str("</div>"); // close #tab-hints
    out.push_str(&ut_html);
    out.push_str(&poptracker_panel(game_id, pt_has_pack, pt_can_manage));
    out.push_str(&log_html);
    out.push_str(TABS_JS);
    out.push_str(GAME_TAB_JS);
    out.push_str(LIVE_JS);

    out.push_str("</main></body></html>");
    Html(out)
}

/// The rows of the per-game hint list ("where this slot's items are"), also
/// served as a fragment for live refresh after a hint.
pub fn hint_rows(
    views: &[crate::ap::HintView],
    found_items: &[crate::ap::HintView],
    prios: &std::collections::HashSet<(i64, i64, i64)>,
    back: &str,
    can_prioritize: bool,
) -> String {
    // Active list = hints whose item is not yet found (these carry the
    // prioritize toggle). The Found section = every item the slot has received
    // (`found_items`), rendered like a found hint; it's a superset of the found
    // hints. Before the savefile has been read `found_items` is empty, so fall
    // back to the found hints from the live tracker.
    let mut active: Vec<&crate::ap::HintView> = views.iter().filter(|h| !h.found).collect();
    let mut found: Vec<&crate::ap::HintView> = if found_items.is_empty() {
        views.iter().filter(|h| h.found).collect()
    } else {
        found_items.iter().collect()
    };
    if active.is_empty() && found.is_empty() {
        return r#"<div class="empty">No hints here yet.</div>"#.to_string();
    }

    // Sort by item name (the `text` starts with it), case-insensitively.
    let name_key = |h: &&crate::ap::HintView| {
        h.text
            .split_once(" @ ")
            .map_or(h.text.as_str(), |(item, _)| item)
            .to_ascii_lowercase()
    };
    let is_prio = |h: &&crate::ap::HintView| prios.contains(&(h.finding, h.location, h.item));
    // Active: prioritized first (false < true, so negate), then alphabetical.
    active.sort_by_key(|h| (!is_prio(h), name_key(h)));
    found.sort_by_key(name_key);

    let mut out = String::new();
    if active.is_empty() {
        out.push_str(r#"<div class="empty">No unfound hints.</div>"#);
    }
    for h in &active {
        let on = prios.contains(&(h.finding, h.location, h.item));
        if !can_prioritize {
            // Non-claimants see the hints but can't reprioritize them; a
            // prioritized one is still marked.
            let star = if on { " ★" } else { "" };
            out.push_str(&format!(
                r#"<div class="hintrow"><span>{}{star}</span></div>"#,
                esc(&h.text)
            ));
            continue;
        }
        let (cls, label) = if on {
            ("", "★ prioritized")
        } else {
            ("ghost", "☆ prioritize")
        };
        out.push_str(&format!(
            r#"<div class="hintrow"><span>{}</span><form method="post" action="prioritize" style="display:inline">
<input type="hidden" name="finding" value="{}"><input type="hidden" name="location" value="{}">
<input type="hidden" name="item" value="{}"><input type="hidden" name="receiving" value="{}">
<input type="hidden" name="back" value="{}">
<button class="{cls}" type="submit">{label}</button></form></div>"#,
            esc(&h.text),
            h.finding,
            h.location,
            h.item,
            h.receiving,
            esc(back),
        ));
    }
    if !found.is_empty() {
        out.push_str(&format!(
            r#"<details class="foundhints"><summary>Found ({})</summary>"#,
            found.len()
        ));
        for h in &found {
            out.push_str(&format!(
                r#"<div class="hintrow found"><span>{}</span></div>"#,
                esc(&h.text)
            ));
        }
        out.push_str("</details>");
    }
    out
}
