// Draws each `data-buddy="pack shape colour state"` mark with the app's own buddy code, cropped as the app crops it.
import { packById } from '../../shared/buddies/catalogue';
import { buddyMarkup } from '../../shared/buddies/render';

document.querySelectorAll('[data-buddy]').forEach((mark, index) => {
  const [packId, shapeId, slot, state] = mark.dataset.buddy.split(' ');
  const pack = packById(packId);
  const shape = pack?.shapes.find((candidate) => candidate.id === shapeId);
  if (!pack || !shape) throw new Error(`screens.src.html names a buddy the app does not have: ${mark.dataset.buddy}`);
  mark.innerHTML = buddyMarkup(pack, shape, pack.colors[Number(slot)], state, `design${index}`);
});
