//! Minesweeper: the game page, stats and leaderboards.

use super::*;

/// A duration in milliseconds as `M:SS`.
pub(super) fn fmt_ms(ms: i64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// One labelled stats block (for a single board kind).
pub(super) fn mine_stats_block(label: &str, st: &crate::db::MineStats) -> String {
    let time = |o: Option<i64>| o.map(fmt_ms).unwrap_or_else(|| "—".to_string());
    let rate = if st.total() == 0 {
        "—".to_string()
    } else {
        format!("{}%", st.success_pct())
    };
    format!(
        r#"<div class="msblock"><h4>{label}</h4>
<div class="mstat"><span>Completed</span><b>{}</b></div>
<div class="mstat"><span>Failures</span><b>{}</b></div>
<div class="mstat"><span>Success rate</span><b>{}</b></div>
<div class="mstat sep"><span>Best time</span><b>{}</b></div>
<div class="mstat"><span>Average time</span><b>{}</b></div>
<div class="mstat"><span>Worst time</span><b>{}</b></div></div>"#,
        st.wins,
        st.losses,
        rate,
        time(st.best_ms),
        time(st.avg_ms),
        time(st.worst_ms),
    )
}

/// The minesweeper leaderboards for one board kind: best time and best
/// completion rate (min-games gated), top N each. `min_games` is shown as a note.
pub fn mine_leaderboard_html(
    kind_label: &str,
    time: &[(String, i64)],
    rate: &[(String, i64, i64)],
    min_games: i64,
) -> String {
    let mut out = format!(r#"<h2 class="lbhead">{} leaderboard</h2>"#, esc(kind_label));

    out.push_str(r#"<h3 class="lbsub">Best time</h3>"#);
    if time.is_empty() {
        out.push_str(r#"<p class="empty">No cleared boards yet.</p>"#);
    } else {
        out.push_str(r#"<table class="lb"><thead><tr><th>#</th><th>Player</th><th>Time</th></tr></thead><tbody>"#);
        for (i, (sso, ms)) in time.iter().enumerate() {
            out.push_str(&format!(
                r#"<tr><td class="rank">{}</td><td>{}</td><td class="num">{}</td></tr>"#,
                i + 1,
                esc(sso),
                fmt_ms(*ms),
            ));
        }
        out.push_str("</tbody></table>");
    }

    out.push_str(&format!(
        r#"<h3 class="lbsub">Best completion rate <small>(min {min_games} games)</small></h3>"#
    ));
    if rate.is_empty() {
        out.push_str(&format!(
            r#"<p class="empty">No one has played {min_games} games yet.</p>"#
        ));
    } else {
        out.push_str(r#"<table class="lb"><thead><tr><th>#</th><th>Player</th><th>Rate</th><th>Games</th></tr></thead><tbody>"#);
        for (i, (sso, wins, total)) in rate.iter().enumerate() {
            let pct = if *total > 0 { wins * 100 / total } else { 0 };
            out.push_str(&format!(
                r#"<tr><td class="rank">{}</td><td>{}</td><td class="num">{}%</td><td class="num">{}</td></tr>"#,
                i + 1,
                esc(sso),
                pct,
                total,
            ));
        }
        out.push_str("</tbody></table>");
    }
    out
}

/// The stats panel body, per board kind — reused for the initial page and the
/// live refresh the reveal endpoint returns when a game ends.
pub fn mine_stats_html(standard: &crate::db::MineStats, expert: &crate::db::MineStats) -> String {
    format!(
        "{}{}",
        mine_stats_block("Standard", standard),
        mine_stats_block("Expert", expert)
    )
}

/// The minesweeper minigame page: a server-authoritative board. `free` is the
/// standalone minigame mode (from the home Minigames section) — no bonus hints,
/// all-time leaderboard/stats; otherwise it's the seasonal, board-linked mode.
pub fn minesweeper_page(
    enabled: bool,
    free: bool,
    credits: i64,
    standard: &crate::db::MineStats,
    expert: &crate::db::MineStats,
    back: &str,
) -> Html<String> {
    let mut b = page_head(None);
    // Freeplay (from the home minigames section) goes home; seasonal goes back to
    // wherever you entered from — the game page you opened minesweeper for, else
    // the board.
    let crumb = if free {
        r#"<div class="crumb"><a href="/">← Bifrost</a></div>"#.to_string()
    } else {
        let label = if back.starts_with("/game/") {
            "← back to game"
        } else {
            "← board"
        };
        format!(
            r#"<div class="crumb"><a href="{}">{label}</a></div>"#,
            esc(back)
        )
    };
    b.push_str(&format!(r#"<h1>Minesweeper</h1>{crumb}"#));
    if !enabled {
        b.push_str(
            r#"<div class="banner bad">The minesweeper minigame is disabled right now.</div></main></body></html>"#,
        );
        return Html(b);
    }
    // Intro + stat scope differ by mode; freeplay drops the "Bonus hints:" tally.
    let intro = if free {
        "Just for fun — results count toward the all-time leaderboards (no bonus hints here). Every board is solvable by logic alone, no guessing needed, on both difficulties. Left-click a cell to reveal, right-click to flag. On a revealed number: left-click chord-reveals its neighbours once you've flagged that many mines, right-click flags all remaining neighbours when they must all be mines."
    } else {
        "Clear the board without hitting a mine to earn a bonus hint — worth 1 either way. Every board is solvable by logic alone, no guessing needed, on both difficulties. Left-click a cell to reveal, right-click to flag. On a revealed number: left-click chord-reveals its neighbours once you've flagged that many mines, right-click flags all remaining neighbours when they must all be mines. Bonus hints are spent automatically when you hint without enough points."
    };
    let statline = if free {
        r#"<div class="statline"><span id="minesleft"></span> · ⏱ <b id="mstimer">0:00</b></div>"#
            .to_string()
    } else {
        format!(
            r#"<div class="statline">Bonus hints: <b id="credits">{credits}</b> · <span id="minesleft"></span> · ⏱ <b id="mstimer">0:00</b></div>"#
        )
    };
    let statnote = if free {
        "All-time, across every season. Times are for cleared boards."
    } else {
        "This season. Times are for cleared boards."
    };
    b.push_str(&format!(
        r#"<p class="empty mineintro">{intro}</p>
<div class="mstabs">
  <button class="mstab active" data-tab="standard">Standard <small>16×16 · 40</small></button>
  <button class="mstab" data-tab="expert">Expert <small>30×16 · 99</small></button>
  <span class="mstabgap"></span>
  <button class="mstab" data-lb="standard">🏆 Standard</button>
  <button class="mstab" data-lb="expert">🏆 Expert</button>
</div>
<div id="playview">
  {statline}
  <div id="msg" class="inlinemsg"></div>
  <div class="minewrap">
    <div class="mineboard">
      <div class="msscroll"><div id="grid" class="msgrid"></div></div>
      <div class="btnrow"><button id="newgame" class="ghost" style="display:none">New game</button></div>
    </div>
    <aside class="minestats-panel">
      <h3>Your stats</h3>
      <div id="minestats" class="minestats">{stats}</div>
      <p class="mstatnote">{statnote}</p>
    </aside>
  </div>
</div>
<div id="lbview" class="lbview" style="display:none"></div>
<script>var FREE={free};</script>"#,
        stats = mine_stats_html(standard, expert),
    ));
    b.push_str(MINE_STYLE);
    b.push_str(MINE_JS);
    b.push_str("</main></body></html>");
    Html(b)
}

pub(super) const MINE_STYLE: &str = r#"<style>
/* Widen this page (and match the header) so the full expert board fits beside
   the stats instead of scrolling inside the 900px default column. */
main{max-width:1180px}
header .hwrap{max-width:1180px}
.mineintro{max-width:860px}
.minewrap{display:flex;gap:18px;align-items:flex-start;flex-wrap:wrap;margin-top:14px}
.mineboard{min-width:0}
.minestats-panel{flex:0 1 220px;min-width:190px;background:var(--panel);border:1px solid var(--bd);border-radius:12px;padding:14px 16px}
.minestats-panel h3{margin:0 0 12px;font-size:12px;text-transform:uppercase;letter-spacing:.09em;color:var(--muted)}
.minestats{display:grid;gap:16px}
.msblock{display:grid;gap:9px}
.msblock h4{margin:0;font-size:11px;text-transform:uppercase;letter-spacing:.08em;color:var(--accent)}
.mstat{display:flex;justify-content:space-between;align-items:baseline;gap:16px;font-size:13.5px}
.mstat>span{color:var(--muted)}
.mstat>b{font-family:var(--mono);font-size:14px}
.mstat.sep{border-top:1px solid var(--bd);padding-top:9px;margin-top:2px}
.mstatnote{color:var(--muted);font-size:11px;margin:14px 0 0;line-height:1.4}
.btnrow{display:flex;gap:10px;flex-wrap:wrap}
.mstabs{display:flex;gap:6px;margin:0 0 12px;flex-wrap:wrap;align-items:center}
.mstab{background:transparent;border:1px solid var(--bd);border-radius:8px;color:var(--muted);font-weight:600;padding:7px 13px;cursor:pointer;font-size:13px}
.mstab small{font-weight:400;opacity:.7;margin-left:6px;font-size:11px}
.mstab:hover{color:var(--fg);border-color:var(--accent)}
.mstab.active{color:#0d1117;background:var(--accent);border-color:var(--accent)}
.mstabgap{width:1px;align-self:stretch;background:var(--bd);margin:2px 4px}
.lbview{margin-top:14px;max-width:560px}
.lbhead{font-size:15px;margin:0 0 6px}
.lbsub{font-size:12px;text-transform:uppercase;letter-spacing:.08em;color:var(--muted);margin:18px 0 8px}
.lbsub small{text-transform:none;letter-spacing:0;font-weight:400}
table.lb{width:100%;border-collapse:collapse;font-size:13.5px}
table.lb th{text-align:left;font-size:11px;text-transform:uppercase;letter-spacing:.06em;color:var(--muted);font-weight:600;padding:4px 10px;border-bottom:1px solid var(--bd)}
table.lb td{padding:6px 10px;border-bottom:1px solid var(--bd)}
table.lb td.rank{color:var(--muted);font-family:var(--mono);width:1%;white-space:nowrap}
table.lb td.num{font-family:var(--mono);text-align:right}
table.lb th:last-child{text-align:right}
table.lb tr:first-child td.rank{color:var(--accent);font-weight:700}
.msscroll{overflow:auto;max-width:100%;margin:0 0 10px}
.msgrid{display:grid;gap:2px;width:max-content}
.cell{width:28px;height:28px;padding:0;background:var(--panel);border:1px solid var(--bd);border-radius:4px;color:var(--fg);font-family:var(--mono);font-size:14px;font-weight:700;cursor:pointer;line-height:1;text-align:center}
.msgrid.exp .cell{width:24px;height:24px;font-size:12px;border-radius:3px}
.cell:hover:not(.open){border-color:var(--accent)}
.cell.open{background:var(--bg);cursor:default}
.cell.mine{background:var(--bad);color:#fff;border-color:var(--bad)}
.cell.flag{color:var(--warn)}
.cell[data-n="1"]{color:#5aa9e6}.cell[data-n="2"]{color:#28a56d}.cell[data-n="3"]{color:#e24d5f}
.cell[data-n="4"]{color:#c58af9}.cell[data-n="5"]{color:#d98324}.cell[data-n="6"]{color:#3fbfbf}
.cell[data-n="7"]{color:#e6e9ef}.cell[data-n="8"]{color:#98a3b6}
</style>"#;

pub(super) const MINE_JS: &str = r#"<script>
(function(){
var W,H,minesN,over=false,flags=0,cells=[],activeKind='standard',view='board',lbKind='standard';
var grid=document.getElementById('grid'),msg=document.getElementById('msg'),
    minesleft=document.getElementById('minesleft'),creditsEl=document.getElementById('credits'),
    statsEl=document.getElementById('minestats'),timerEl=document.getElementById('mstimer'),
    newBtn=document.getElementById('newgame'),tabs=document.querySelectorAll('.mstab'),
    playview=document.getElementById('playview'),lbview=document.getElementById('lbview');
// Each difficulty is its own persistent board (keyed server-side by player+kind),
// so switching tabs keeps each game + timer alive. The "New game" button only
// shows once the current board ends (to replay the same difficulty).
function showNewBtn(s){if(newBtn)newBtn.style.display=s?'':'none';}
function updateTabUI(){tabs.forEach(function(t){
 var on=view==='board'?(t.getAttribute('data-tab')===activeKind):(t.getAttribute('data-lb')===lbKind);
 t.classList.toggle('active',on);
});}
// Show a season leaderboard tab (pausing the live board we're leaving).
function showLb(kind){
 if(view==='board'&&!over)pauseKind(activeKind);
 view='lb';lbKind=kind;playview.style.display='none';lbview.style.display='';updateTabUI();
 lbview.innerHTML='<p class="empty">Loading…</p>';
 fetch('minesweeper/leaderboard?kind='+kind+FB).then(function(r){return r.text()}).then(function(h){
  if(view==='lb'&&lbKind===kind)lbview.innerHTML=h;
 }).catch(function(){lbview.innerHTML='<p class="empty">Could not load leaderboard.</p>';});
}
// Timer: mirrors the server's active-play clock (which excludes time while the
// tab is hidden/closed). baseMs is the last value the server reported; while
// running we add the wall-clock time since that sync.
var baseMs=0,running=false,syncAt=0;
function fmtTime(ms){var s=Math.max(0,Math.floor(ms/1000));return Math.floor(s/60)+':'+('0'+(s%60)).slice(-2);}
function renderTimer(){if(timerEl)timerEl.textContent=fmtTime(baseMs+(running?(Date.now()-syncAt):0));}
function setTimer(ms,isRunning){if(ms!=null)baseMs=ms;running=!!isRunning;syncAt=Date.now();renderTimer();}
setInterval(renderTimer,250);
// Freeplay (minigame section) vs seasonal (board) — thread the mode through
// every request so the server records to the right scope and skips bonus hints.
// `FREE` is the page-level global set just above this script.
var IS_FREE=(typeof FREE!=='undefined')&&FREE===true;
var FB=IS_FREE?'&free=true':'';
function wf(o){if(IS_FREE)o.free='true';return o;}
function post(url,body){return fetch(url,{method:'POST',body:body?new URLSearchParams(body):undefined}).then(function(r){return r.json()});}
function setStats(h){if(h!=null&&statsEl)statsEl.innerHTML=h;}
function openCell(cell){var cb=cells[cell.r][cell.c];cb.classList.remove('flag');cb.classList.add('open');if(cell.n>0){cb.textContent=cell.n;cb.setAttribute('data-n',cell.n);}else{cb.textContent='';}}
function updateMines(){if(minesleft)minesleft.textContent=(minesN-flags)+' mines';}
function inb(r,c){return r>=0&&r<H&&c>=0&&c<W;}
function nbrs(r,c){var out=[];for(var dr=-1;dr<=1;dr++)for(var dc=-1;dc<=1;dc++){if((dr||dc)&&inb(r+dr,c+dc))out.push([r+dr,c+dc]);}return out;}
function numOf(btn){return btn.classList.contains('open')?(parseInt(btn.textContent,10)||0):-1;}
function surround(r,c){var flagged=0,hidden=[];nbrs(r,c).forEach(function(p){var b=cells[p[0]][p[1]];if(b.classList.contains('flag'))flagged++;else if(!b.classList.contains('open'))hidden.push(p);});return{flagged:flagged,hidden:hidden};}
function buildGrid(){
 var exp=activeKind==='expert',cs=exp?24:28;
 grid.className='msgrid'+(exp?' exp':'');
 grid.innerHTML='';grid.style.gridTemplateColumns='repeat('+W+','+cs+'px)';cells=[];
 for(var r=0;r<H;r++){var row=[];for(var c=0;c<W;c++){
  var btn=document.createElement('button');btn.className='cell';
  (function(r,c,btn){
   btn.addEventListener('click',function(){if(btn.classList.contains('open'))chordReveal(r,c);else reveal(r,c);});
   btn.addEventListener('contextmenu',function(e){e.preventDefault();if(btn.classList.contains('open'))chordFlag(r,c);else toggleFlag(btn);});
  })(r,c,btn);
  grid.appendChild(btn);row.push(btn);
 }cells.push(row);}
}
function toggleFlag(btn){
 if(over||btn.classList.contains('open'))return;
 if(btn.classList.contains('flag')){btn.classList.remove('flag');btn.textContent='';flags--;}
 else{btn.classList.add('flag');btn.textContent='⚑';flags++;}
 updateMines();
}
// Right-click a number: if the remaining hidden neighbours must all be mines, flag them.
function chordFlag(r,c){
 if(over)return;var n=numOf(cells[r][c]);if(n<=0)return;
 var s=surround(r,c);
 if(s.hidden.length>0&&s.hidden.length===n-s.flagged){
  s.hidden.forEach(function(p){var b=cells[p[0]][p[1]];if(!b.classList.contains('flag')){b.classList.add('flag');b.textContent='⚑';flags++;}});
  updateMines();
 }
}
// Left-click a number: once you've flagged exactly that many, reveal the rest.
function chordReveal(r,c){
 if(over)return;var n=numOf(cells[r][c]);if(n<=0)return;
 var s=surround(r,c);
 if(s.flagged!==n)return;
 var q=s.hidden.slice();
 (function step(){
  if(over||!q.length)return;
  var p=q.shift();
  if(cells[p[0]][p[1]].classList.contains('open')){step();return;}
  reveal(p[0],p[1]).then(step);
 })();
}
function reveal(r,c){
 if(over)return Promise.resolve();var btn=cells[r][c];
 if(btn.classList.contains('open')||btn.classList.contains('flag'))return Promise.resolve();
 return post('minesweeper/reveal',wf({r:r,c:c,kind:activeKind})).then(function(j){
  if(over)return;
  if(!j.ok){msg.className='inlinemsg bad';msg.textContent=j.error||'Invalid.';return;}
  if(j.state==='lost'){over=true;(j.mines||[]).forEach(function(m){var mb=cells[m.r][m.c];mb.classList.add('open','mine');mb.textContent='✱';});msg.className='inlinemsg bad';msg.textContent='Boom — try again.';setStats(j.stats);setTimer(j.elapsed_ms,false);showNewBtn(true);return;}
  (j.cells||[]).forEach(openCell);
  setTimer(j.elapsed_ms,j.running);
  if(j.state==='won'){over=true;msg.className='inlinemsg ok';msg.textContent='Cleared! You earned a bonus hint.';if(j.credits!=null&&creditsEl)creditsEl.textContent=j.credits;setStats(j.stats);showNewBtn(true);}
 }).catch(function(){if(!over){msg.className='inlinemsg bad';msg.textContent='Request failed.';}});
}
function startBoard(w,h,mn){W=w;H=h;minesN=mn;over=false;flags=0;buildGrid();updateMines();}
// Start a fresh board for the active difficulty.
function newGame(){
 msg.textContent='';msg.className='inlinemsg';showNewBtn(false);
 post('minesweeper/new',wf({kind:activeKind})).then(function(j){
  if(!j.ok){msg.className='inlinemsg bad';msg.textContent=j.error||'Could not start.';grid.innerHTML='';return;}
  startBoard(j.w,j.h,j.mines);setTimer(0,false);
 }).catch(function(){msg.className='inlinemsg bad';msg.textContent='Could not start.';});
}
// Show the active difficulty's board: resume the in-progress one (survives tab
// close/reopen and difficulty switches) or auto-start a fresh one. The timer
// comes back paused and resumes on the next reveal; an untouched board shows no
// "resumed" notice.
function loadGame(){
 fetch('minesweeper/state?kind='+activeKind+FB).then(function(r){return r.json()}).then(function(j){
  if(!j||!j.active){newGame();return;}
  startBoard(j.w,j.h,j.mines);showNewBtn(false);
  var cs=j.cells||[];cs.forEach(openCell);setTimer(j.elapsed_ms,false);
  msg.className='inlinemsg';msg.textContent=cs.length?'Resumed your board — click to continue.':'';
 }).catch(function(){newGame();});
}
// Pause the active difficulty's clock (tab hidden/closed, or leaving for the
// other difficulty); it resumes on the next reveal of that board.
function pauseKind(kind){
 try{if(!navigator.sendBeacon('minesweeper/pause?kind='+kind+FB))post('minesweeper/pause?kind='+kind+FB);}catch(e){post('minesweeper/pause?kind='+kind+FB);}
}
function pauseNow(){
 if(view!=='board'||over)return;
 setTimer(baseMs+(running?(Date.now()-syncAt):0),false);
 pauseKind(activeKind);
}
function selectTab(kind){
 if(view==='board'&&kind===activeKind){if(over)newGame();return;}
 if(view==='board'&&!over)pauseKind(activeKind); // bank the board we're leaving
 view='board';activeKind=kind;lbview.style.display='none';playview.style.display='';
 updateTabUI();showNewBtn(false);loadGame();
}
document.addEventListener('visibilitychange',function(){if(document.visibilityState==='hidden')pauseNow();});
window.addEventListener('pagehide',pauseNow);
if(newBtn)newBtn.addEventListener('click',newGame);
tabs.forEach(function(t){t.addEventListener('click',function(){
 var lk=t.getAttribute('data-lb');
 if(lk)showLb(lk);else selectTab(t.getAttribute('data-tab'));
});});
updateTabUI();
loadGame();
})();
</script>"#;
