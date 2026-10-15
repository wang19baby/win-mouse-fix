// The server owns the OS button. This controller owns only the current touch
// request; transport loss always stops renewal and lets the server reclaim it.
(function (global) {
  'use strict';
  global.makeDragFactory = function (deps) {
    var nextId = 0, id = null, phase = 'idle', heartbeat = null;
    function send(message) {
      if (!deps.isAuthed()) return false;
      try { return deps.getSend()(message) !== false; } catch (_) { return false; }
    }
    function stopHeartbeat() {
      clearInterval(heartbeat);
      heartbeat = null;
    }
    function idle() { stopHeartbeat(); id = null; phase = 'idle'; }
    function label(text) { deps.updateHud(0, text); }
    function cancel(reason) {
      stopHeartbeat();
      var ending = id;
      var needsEnd = phase === 'pending' || phase === 'active';
      if (needsEnd) phase = 'ending'; // Invalidate before releasing capture.
      deps.onCancel(); // Clears longpress, pointers and pending movement.
      if (needsEnd && !send({ t: 'drag', action: 'end', id: ending })) {
        idle();
        label('连接中断；服务端将回收拖选');
      } else if (needsEnd) label('正在释放拖选');
    }
    function disconnected() { cancel('disconnected'); idle(); }
    function startDrag() {
      if (phase !== 'idle' || !deps.isAuthed() || document.hidden || nextId >= Number.MAX_SAFE_INTEGER) return false;
      id = ++nextId;
      phase = 'pending';
      if (!send({ t: 'drag', action: 'start', id: id })) { idle(); return false; }
      label('等待拖选确认');
      heartbeat = setInterval(function () {
        if (phase !== 'pending' && phase !== 'active') return;
        if (document.hidden || !send({ t: 'drag', action: 'renew', id: id })) cancel('unavailable');
      }, 250);
      return true;
    }
    function moveDrag(dx, dy) {
      if (phase !== 'pending' && phase !== 'active') return false;
      if (!send({ t: 'move', dx: dx, dy: dy, drag_id: id })) { cancel('unavailable'); return false; }
      return true;
    }
    // The page flushes its accelerated rAF motion BEFORE this call. Keep the id
    // while ending so a failed release remains visible and a late start cannot
    // reactivate the touch or restart its heartbeat.
    function endDrag() {
      if (phase !== 'pending' && phase !== 'active') return;
      stopHeartbeat();
      phase = 'ending';
      label('正在释放拖选');
      if (!send({ t: 'drag', action: 'end', id: id })) { idle(); label('连接中断；服务端将回收拖选'); }
    }
    function onServerMessage(message) {
      if (message.t !== 'drag' || message.id !== id) return;
      if (message.state === 'started') {
        if (phase === 'pending') { phase = 'active'; label('拖动 / 选字'); }
      } else if (message.state === 'ended') {
        var interrupted = phase === 'pending' || phase === 'active';
        idle();
        if (interrupted) deps.onCancel();
        label('待机');
      } else if (message.state === 'rejected') {
        idle(); deps.onCancel(); label('拖选被拒绝：' + (message.reason || '无法按下左键'));
      } else if (message.state === 'release_failed') {
        stopHeartbeat(); phase = 'blocked'; deps.onCancel();
        label('左键释放受阻；请检查桌面权限');
      }
    }
    return {
      startDrag: startDrag,
      moveDrag: moveDrag,
      endDrag: endDrag,
      onServerMessage: onServerMessage,
      isDragging: function () { return phase === 'pending' || phase === 'active'; },
      isBlocked: function () { return phase === 'ending' || phase === 'blocked'; },
      cancel: cancel,
      onLostCapture: function () { cancel('capture_lost'); },
      onSecondPointer: function () { cancel('second_pointer'); },
      onBlur: function () { cancel('blur'); },
      onVisibilityHidden: function () { cancel('hidden'); },
      onWsClose: disconnected,
      onWsError: disconnected,
      onWsReject: disconnected
    };
  };
})(window);
