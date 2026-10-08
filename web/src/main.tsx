import { render } from 'solid-js/web';
import { follow } from './api';
import { App } from './App';
import { load, reload, tick } from './state';
import './styles.css';

render(() => <App />, document.getElementById('root')!);

const changes = new BroadcastChannel('remuda-changes');
changes.onmessage = reload;
const changed = () => {
  changes.postMessage(null);
  reload();
};

load();
// Browsers allow a handful of connections per host across all tabs, so one
// tab holds the stream and tells the others.
if (navigator.locks) navigator.locks.request('remuda-events', () => follow(changed));
else follow(changed);
setInterval(() => {
  tick();
  reload();
}, 60000);
