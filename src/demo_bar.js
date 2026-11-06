
// Demo build only (src/demo.rs): the bar that switches between the demo
// accounts, says when the demo resets, and greys out what the demo turns off.
// Built from DOM nodes, like the rest of the header.
(function(){
 var roles=['player','moderator','sysadmin'];
 // Cosmetic only: src/demo.rs refuses these whatever the page shows.
 var locked=[/^\/admin\/settings(\/|$)/,/^\/admin\/catalog\//,/^\/admin\/common-games(\/|$)/,/^\/admin\/users\//,/^\/admin\/links\//,/^\/admin\/worlds\/(approve|to-catalog)$/,/^\/signup\/apworld$/,/^\/admin\/seasons\/import$/,/^\/game\/[^\/]+\/poptracker$/];
 function tail(u){var p=new URL(u,location.href).pathname;var m=p.match(/^\/s\/[0-9a-z]+(\/.*)?$/);return m?(m[1]||'/board'):p}
 function isLocked(u){var t=tail(u);return locked.some(function(r){return r.test(t)})}
 // What the server still takes on the demo's own seasons (src/demo.rs, allowed_on_own).
 var ownOk=[/^\/(hint|prioritize|signup|signup\/check|signup\/delete|admin\/console)$/,/^\/minesweeper\//,/^\/seat\//,/^\/common\/[^\/]+\/ident$/,/^\/signup\/game\/[^\/]+\/(notes|yaml)$/];
 function mark(prot){
  // The demo's own seasons: their admin forms (bar the console) are refused too.
  var sm=location.pathname.match(/^\/s\/([0-9a-z]+)\//);
  if(sm&&prot&&prot.indexOf(sm[1])>=0){
   document.querySelectorAll('form').forEach(function(f){
    var t=tail(f.action);
    if((f.method||'get').toLowerCase()==='post'&&!ownOk.some(function(r){return r.test(t)}))
     f.querySelectorAll('button,input[type=submit],input[type=file]').forEach(function(b){b.disabled=true;b.title="The demo's own seasons stay as they are: make a season of your own to try this"});
   });
  }
  document.querySelectorAll('form').forEach(function(f){
   // f.action is resolved against the page's <base>, as the browser would.
   if((f.method||'get').toLowerCase()!=='post'||!isLocked(f.action))return;
   f.querySelectorAll('button,input[type=submit],input[type=file]').forEach(function(b){b.disabled=true;b.title='Turned off in the demo'});
  });
  document.querySelectorAll('a[href*="/backups/download"]').forEach(function(a){a.removeAttribute('href');a.title='Turned off in the demo'});
 }
 function bar(user,reset){
  var old=document.getElementById('demobar');if(old)old.remove();
  var d=document.createElement('div');d.id='demobar';
  d.style.cssText='position:fixed;right:12px;bottom:12px;max-width:calc(100vw - 24px);z-index:9999;display:flex;align-items:center;gap:4px;padding:4px 4px 4px 12px;border-radius:999px;background:#000d;border:1px solid #7b8cff88;font:600 12px system-ui,sans-serif;color:#aab;box-shadow:0 2px 12px #0009';
  var left=reset==null?'':(reset<90?'1 min':Math.round(reset/60)+' min');
  var t=document.createElement('span');
  t.textContent='Demo'+(left?' · '+left:'');t.style.cssText='margin-right:4px;white-space:nowrap';
  t.title='A demo build: switch accounts with the buttons. '+(left?'It resets to its starting state in '+left+'. ':'')+'Game servers are only reachable from the demo itself, and some console actions are turned off.';
  d.appendChild(t);
  var rd=location.pathname+location.search;
  roles.forEach(function(r){
   // A POST, which src/demo.rs requires: no link elsewhere can switch you.
   var a=document.createElement('a');a.textContent=r;a.title='View the demo as its '+r;a.href='#';
   a.onclick=function(e){e.preventDefault();var f=document.createElement('form');f.method='post';f.action='/demo/as/'+r+'?rd='+encodeURIComponent(rd);document.body.appendChild(f);f.submit()};
   a.style.cssText='white-space:nowrap;padding:5px 11px;border-radius:999px;text-decoration:none;'+(user===r?'background:#7b8cff;color:#000':'color:#dde');
   d.appendChild(a);
  });
  document.body.appendChild(d);
  // Room for the bar below the page's last line.
  document.body.style.paddingBottom='64px';
 }
 function go(){
  Promise.all([
   fetch('/api/me').then(function(r){return r.json()}).catch(function(){return null}),
   fetch('/demo/status').then(function(r){return r.json()}).catch(function(){return null})
  ]).then(function(v){mark(v[1]&&v[1].protected);bar(v[0]&&v[0].user,v[1]&&v[1].reset_in)});
 }
 if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',go);else go();
})();
