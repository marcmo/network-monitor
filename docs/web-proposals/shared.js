/* Illustrative fixtures shared by three throwaway HTML design proposals. */
(function () {
  const sessions = [
    { id: 'current', name: 'Berlin to Leipzig', date: 'Today, 09:12', duration: '1h 24m', state: 'Recording', healthy: '94.2%', latency: '42' },
    { id: 'yesterday', name: 'Berlin to Leipzig', date: 'Yesterday, 16:40', duration: '1h 18m', state: 'Complete', healthy: '91.8%', latency: '56' },
    { id: 'return', name: 'Leipzig to Berlin', date: '8 Oct, 17:05', duration: '1h 31m', state: 'Complete', healthy: '96.1%', latency: '34' }
  ];

  function chart({ dark = false, kind = 'latency', height = 210, range = '5m' } = {}) {
    const ink = dark ? '#83918f' : '#808b85';
    const grid = dark ? '#293431' : '#e5e9e3';
    const color = dark ? '#9bbec1' : '#44695c';
    const traffic = kind !== 'latency';
    const values = Array.from({ length: 101 }, (_, i) => {
      const wave = Math.sin(i * 1.17) * 8 + Math.cos(i * .43) * 12 + Math.sin(i * 2.14) * 5;
      const spike = i === 27 ? 172 : i === 28 ? 102 : i === 65 ? 220 : i === 66 ? 117 : 0;
      return Math.max(13, 39 + wave + spike);
    });
    const y = v => 181 - Math.min(290, v) / 290 * 151;
    const path = (start, end) => values.slice(start, end).map((v, index) => `${index ? 'L' : 'M'}${48 + (start + index) * 8.98},${y(v).toFixed(1)}`).join(' ');
    const lines = [[0, 48], [52, 80], [83, 101]].map(([start, end]) => `<path d="${path(start, end)}" fill="none" stroke="${color}" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>`).join('');
    return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 980 225" width="100%" height="${height}" role="img" aria-label="Illustrative ${traffic ? 'passive traffic' : 'TCP connection time'} history with a timeout and a missing observation interval" style="display:block;overflow:visible;font-family:inherit">
      ${[40, 90, 140, 181].map((yy, i) => `<line x1="48" x2="950" y1="${yy}" y2="${yy}" stroke="${grid}"/><text x="0" y="${yy + 4}" fill="${ink}" font-size="11">${traffic ? ['15', '10', '5', '0'][i] : ['300', '200', '100', '0'][i]}</text>`).join('')}
      <rect x="479" y="26" width="37" height="155" fill="${dark ? '#402f28' : '#fbede4'}"/><rect x="768" y="26" width="27" height="155" fill="${dark ? '#293431' : '#eceeea'}"/>
      ${!traffic ? `<line x1="48" x2="950" y1="51" y2="51" stroke="${dark ? '#75634b' : '#cdbd9e'}" stroke-dasharray="3 5"/><text x="946" y="43" text-anchor="end" fill="${ink}" font-size="10">250 ms slow threshold</text>` : ''}
      ${lines}<circle cx="946" cy="${y(values[100])}" r="3.5" fill="${color}"/>
      <text x="48" y="211" fill="${ink}" font-size="11">${range === '30m' ? '30 min ago' : '5 min ago'}</text><text x="330" y="211" fill="${ink}" font-size="11">${range === '30m' ? '-20 min' : '-3 min'}</text><text x="635" y="211" fill="${ink}" font-size="11">${range === '30m' ? '-10 min' : '-1 min'}</text><text x="946" y="211" text-anchor="end" fill="${ink}" font-size="11">Now</text>
      <text x="498" y="18" text-anchor="middle" fill="${ink}" font-size="10">Timeout</text><text x="781" y="18" text-anchor="middle" fill="${ink}" font-size="10">No sample</text>
    </svg>`;
  }

  function map({ dark = false, id = 'map', compact = false, session = 'current', route = true, coverage = true } = {}) {
    const p = dark ? {
      bg: '#1e292c', forest: '#253a35', forest2: '#293d37', water: '#304b54', road: '#3b4746', minor: '#303c3d', label: '#aab9b5', city: '#e2e8e4', good: '#81b69b', slow: '#d8af72', bad: '#cc8373', missing: '#819294'
    } : {
      bg: '#f0f0e8', forest: '#e0e7d8', forest2: '#e6ebdd', water: '#cadde0', road: '#ffffff', minor: '#dedfd3', label: '#829086', city: '#475a4e', good: '#5b8874', slow: '#c19a52', bad: '#bd7864', missing: '#b4b9ad'
    };
    const full = 'M 802 111 C 764 123 729 141 699 168 S 658 216 626 241 S 595 265 572 293 S 537 333 510 354 S 457 397 429 426 S 385 477 350 511';
    const sections = [
      {d:'M 802 111 C 764 123 729 141 699 168 S 658 216 626 241',color:p.good,label:'Healthy: fresh successful checks'},
      {d:'M 626 241 C 607 256 595 269 582 282',color:p.slow,label:'Slow: connection time above threshold'},
      {d:'M 582 282 Q 577 286 572 293',color:p.bad,label:'Offline: recent independent TCP failures'},
      {d:'M 572 293 C 553 318 537 333 510 354 S 473 384 457 400',color:p.good,label:'Healthy: fresh successful checks'},
      {d:'M 457 400 Q 442 413 429 426',color:p.missing,label:'No usable location: route gap',gap:true},
      {d:'M 429 426 C 410 449 385 477 350 511',color:session === 'yesterday' ? p.slow : p.good,label:session === 'yesterday' ? 'Slow checks' : 'Healthy checks'}
    ];
    const towns = [
      [802, 111, 'Berlin', true, 16, -13], [665, 157, 'Potsdam', false, -42, 25], [493, 113, 'Brandenburg', false, -70, -10],
      [572, 293, 'Lutherstadt Wittenberg', true, 19, 1], [422, 356, 'Dessau', false, -48, -12],
      [218, 440, 'Halle (Saale)', false, -70, -10], [350, 511, 'Leipzig', true, 20, 12],
      [208, 228, 'Magdeburg', false, -76, -9], [770, 357, 'Luckau', false, 12, -5], [756, 497, 'Torgau', false, 12, -8]
    ];
    return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1000 610" width="100%" role="img" aria-label="Illustrative Berlin to Leipzig route map. Green healthy, amber slow, red offline, dashed missing location. This is sample data, not measured geographic coverage." style="display:block;width:100%;height:100%;min-height:${compact ? '220' : '290'}px;font-family:inherit;background:${p.bg}">
      <defs><pattern id="${id}-grain" width="18" height="18" patternUnits="userSpaceOnUse"><circle cx="2" cy="3" r=".55" fill="${p.label}" opacity=".16"/></pattern></defs>
      <rect width="1000" height="610" fill="${p.bg}"/>
      <g fill="${p.forest}" opacity=".9">
        <path d="M18 32L91 8 149 30 152 68 194 96 180 146 128 158 109 131 71 142 27 105Z"/>
        <path d="M363 32L410 6 472 19 490 58 463 87 431 65 390 90 367 70Z"/>
        <path d="M693 15L746 28 772 65 736 94 681 87 648 58Z"/>
        <path d="M822 190L881 160 942 173 989 212 970 274 916 300 871 262 828 275 800 234Z"/>
        <path d="M441 176L513 154 566 184 572 220 541 250 493 233 467 265 414 240 403 203Z"/>
        <path d="M207 294L274 267 309 283 318 320 286 359 230 346 198 371 174 333Z"/>
        <path d="M568 407L636 378 685 401 721 451 693 482 645 457 603 482 565 453Z"/>
        <path d="M53 468L120 439 156 466 161 505 139 532 89 543 52 518Z"/>
        <path d="M443 532L489 510 529 537 542 584 496 610 440 585Z"/>
      </g>
      <g fill="${p.forest2}"><path d="M254 48L309 25 355 64 332 108 282 116 252 89Z"/><path d="M723 227L770 239 781 282 751 303 710 276Z"/><path d="M848 435L909 405 954 425 975 472 940 514 884 491Z"/><path d="M217 541L256 527 285 553 274 591 241 604 212 575Z"/></g>
      <g fill="none" stroke="${p.water}" stroke-linecap="round" stroke-linejoin="round">
        <path d="M980 512Q920 490 858 462T778 421T701 388T624 355T552 328T485 309T412 282T345 260T279 238T219 196T192 120T177 38" stroke-width="5"/>
        <path d="M964 74Q899 95 846 124T763 132T699 155T629 148T554 131T471 140T380 166" stroke-width="3.5"/>
        <path d="M350 610Q326 557 281 514T225 445T257 365T293 278" stroke-width="2.5"/>
      </g>
      <g fill="${p.water}"><path d="M738 133Q721 114 701 132T699 167Q716 181 727 162Z"/><path d="M909 83Q933 68 946 81L962 116 947 126 931 102Z"/><path d="M796 86L821 79 833 89 814 101Z"/><path d="M333 565Q341 548 351 560L354 581 341 586Z"/></g>
      <g stroke="${p.minor}" fill="none" stroke-width="1.1">
        <path d="M28 228Q198 201 355 173T625 139T974 73"/><path d="M0 430Q147 399 251 388T486 355T804 321T1000 305"/><path d="M106 0Q144 111 223 202T375 377T471 610"/><path d="M560 0Q569 101 598 203T662 421T719 610"/><path d="M998 559Q775 535 587 499T222 456T0 445"/>
        <path d="M410 0Q370 169 351 260T273 432T209 610"/><path d="M850 0Q802 190 823 310T929 610"/><path d="M0 340Q151 311 277 332T456 401T649 432T1000 421"/>
      </g>
      <g stroke="${p.road}" fill="none" stroke-width="3.3" stroke-linecap="round">
        <path d="M-20 190Q233 116 434 104T665 150T802 111L1010 5"/><path d="M210 228Q328 275 414 352T512 430T802 542"/><path d="M200 622Q294 503 382 427T551 299T673 199T813 111L927 -10"/><path d="M217 440Q326 473 350 511T480 596"/><path d="M802 111Q860 277 773 359T756 497L825 620"/>
      </g>
      <rect width="1000" height="610" fill="url(#${id}-grain)" pointer-events="none"/>
      <g fill="${p.label}" font-size="11" letter-spacing="3" opacity=".68"><text x="714" y="220">BRANDENBURG</text><text x="88" y="307">SAXONY-ANHALT</text><text x="479" y="575">SAXONY</text></g>
      <g data-layer="coverage" style="display:${coverage ? 'inline' : 'none'}" fill="none" stroke-linecap="round" opacity=".2">${sections.filter(s => !s.gap).map(s => `<path d="${s.d}" stroke="${s.color}" stroke-width="23"/>`).join('')}</g>
      <g data-layer="route" style="display:${route ? 'inline' : 'none'}" fill="none" stroke-linecap="round" stroke-linejoin="round">
        <path d="${full}" stroke="${p.bg}" stroke-width="9"/>
        <path data-main-route="true" d="${full}" stroke="none"/>
        ${sections.map(s => `<path d="${s.d}" stroke="${s.color}" stroke-width="${s.gap ? '3' : '4.5'}" ${s.gap ? 'stroke-dasharray="3 7"' : ''}><title>${s.label}</title></path>`).join('')}
      </g>
      ${towns.map(([x,y,name,big,dx,dy]) => `<g><circle cx="${x}" cy="${y}" r="${big ? '4.5' : '2.5'}" fill="${big ? p.city : p.label}" stroke="${p.bg}" stroke-width="2"/><text x="${x + dx}" y="${y + dy}" fill="${big ? p.city : p.label}" font-size="${big ? '16' : '12'}" font-weight="${big ? '600' : '400'}" paint-order="stroke" stroke="${p.bg}" stroke-width="4" stroke-linejoin="round">${name}</text></g>`).join('')}
      <g class="map-position" transform="translate(${session === 'return' || session === 'eighth' ? '802 111' : '510 354'})"><circle r="16" fill="${p.good}" opacity=".16"/><circle r="7" fill="${p.good}" stroke="${dark ? '#ecf1e9' : '#fff'}" stroke-width="3"/><title>Selected sample position</title></g>
      <g transform="translate(953 37)" fill="${p.label}" text-anchor="middle"><text y="-13" font-size="10">N</text><path d="M0 -4L4 9 0 6 -4 9Z"/></g>
      <g transform="translate(33 565)" fill="${p.label}" font-size="10"><path d="M0 0V4H65V0" fill="none" stroke="${p.label}"/><text y="20">Illustrative geography</text></g>
      <text x="968" y="588" text-anchor="end" fill="${p.label}" font-size="10">Sample route / not a navigation map</text>
    </svg>`;
  }

  function setMapProgress(container, fraction) {
    const path = container.querySelector('[data-main-route]');
    const marker = container.querySelector('.map-position');
    if (!path || !marker) return;
    const point = path.getPointAtLength(path.getTotalLength() * Math.max(0, Math.min(1, fraction)));
    marker.setAttribute('transform', `translate(${point.x} ${point.y})`);
  }

  function probes() {
    return [['TCP', '1.1.1.1:443', '42 ms', '1 s ago'], ['TCP', '8.8.8.8:443', '38 ms', '1 s ago'], ['DNS', 'example.com', '18 ms', '6 s ago'], ['HTTPS', 'gstatic.com', '64 ms', '12 s ago']].map(row => `<div class="probe-row">${row.map(v => `<span>${v}</span>`).join('')}</div>`).join('');
  }

  window.Mock = { sessions, chart, map, probes, setMapProgress };
})();
