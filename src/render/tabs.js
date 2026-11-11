// Tab bars (`.gtabs`) and their panels (`.gtabpanel`), included in a page by
// `TABS_JS` in mod.rs. Tested by tests/js/tabs.test.mjs.
(function(){var bar=document.querySelector('.gtabs');if(!bar)return;
function activate(id,push){var btn=bar.querySelector('.gtab[data-tab="'+id+'"]');if(!btn)return;
bar.querySelectorAll('.gtab').forEach(function(x){x.classList.toggle('on',x===btn);});
document.querySelectorAll('.gtabpanel').forEach(function(p){p.hidden=(p.id!==id);});
if(push)history.replaceState(null,'','#'+id);
document.querySelectorAll('form').forEach(function(f){var a=f.getAttribute('action');if(a)f.setAttribute('action',a.split('#')[0]+'#'+id);});}
bar.addEventListener('click',function(e){var b=e.target.closest('.gtab');if(!b)return;activate(b.getAttribute('data-tab'),true);});
document.querySelectorAll('.gtabpanel a[href]').forEach(function(a){if(a.getAttribute('href').charAt(0)==='#')return;var u=new URL(a.href);
if(u.origin===location.origin&&u.pathname===location.pathname&&!u.hash){u.hash=a.closest('.gtabpanel').id;a.href=u.href;}});
var h=(location.hash||'').slice(1),el=h&&document.getElementById(h);
if(el&&el.classList.contains('gtabpanel'))activate(h,false);
})();
