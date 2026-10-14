/**
 * Deluge.ArchiveInfoWindow.js
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Everything the archive knows about one torrent, and what rclone says about
 * its files, in one window: where it is, what its label asks for, the rule's
 * own flags, rclone's queue entries (tries, next attempt) and the transfer in
 * progress (percentage, speed, time left). Opened from the torrent's menu,
 * *Archive Info*; it refreshes itself while open.
 *
 * The figures come from `redeluge.get_torrent_archive`.
 */
Ext.ns('Deluge');

/**
 * @class Deluge.ArchiveInfoWindow
 * @extends Ext.Window
 */
Deluge.ArchiveInfoWindow = Ext.extend(Ext.Window, {
    title: _('Archive Info'),
    width: 680,
    height: 520,
    layout: 'fit',
    closeAction: 'hide',
    constrainHeader: true,
    plain: true,
    minWidth: 480,
    minHeight: 300,

    /** Seconds between refreshes while open: an upload moves quickly. */
    interval: 3,

    initComponent: function () {
        Deluge.ArchiveInfoWindow.superclass.initComponent.call(this);
        this.info = this.add({
            xtype: 'panel',
            border: false,
            autoScroll: true,
            bodyStyle: 'padding: 10px;',
            html: '',
        });
        this.addButton(_('Refresh'), this.load, this);
        this.addButton(_('Close'), this.hide, this);
        this.on('hide', this.stopPolling, this);
        this.on('destroy', this.stopPolling, this);
    },

    /** Shows the window for one torrent. */
    showFor: function (torrentId) {
        this.torrentId = torrentId;
        this.show();
        this.info.update('<i>' + _('Loading…') + '</i>');
        this.load();
        this.stopPolling();
        this.timer = window.setInterval(
            this.load.createDelegate(this),
            this.interval * 1000
        );
    },

    stopPolling: function () {
        if (!this.timer) return;
        window.clearInterval(this.timer);
        this.timer = null;
    },

    load: function () {
        if (!this.torrentId) return;
        deluge.client.redeluge.get_torrent_archive(this.torrentId, {
            success: function (info) {
                if (!this.isVisible()) return;
                this.setTitle(String.format(_('Archive Info: {0}'), info.name || ''));
                this.info.update(this.renderInfo(info));
            },
            failure: function () {
                if (!this.isVisible()) return;
                this.info.update(
                    '<i>' + _('The daemon did not answer, or the torrent is gone.') + '</i>'
                );
            },
            scope: this,
        });
    },

    renderInfo: function (info) {
        var esc = Ext.util.Format.htmlEncode;
        var yes = _('yes');
        var no = _('no');
        var orange = 'color: #f0883e; font-weight: bold;';
        var section = function (title, rows) {
            var body = rows
                .filter(function (row) {
                    return row && row[1] !== undefined && row[1] !== null && row[1] !== '';
                })
                .map(function (row) {
                    return (
                        '<tr><td style="opacity: 0.72; padding: 1px 14px 1px 0; white-space: nowrap; vertical-align: top;">' +
                        row[0] +
                        '</td><td>' +
                        row[1] +
                        '</td></tr>'
                    );
                })
                .join('');
            return (
                '<div style="margin-bottom: 12px;"><b>' +
                title +
                '</b><table style="margin: 3px 0 0 8px;">' +
                body +
                '</table></div>'
            );
        };
        var view = Deluge.archiveView({
            archive_state: info.archive_state,
            state: info.state,
        });
        var flags = info.flags || {};
        var since = function (seconds) {
            return seconds >= 0 ? String.format(_('{0} ago'), ftime(seconds)) : _('never');
        };

        var html = section(_('Torrent'), [
            [_('State'), esc(info.state)],
            [_('Archive'), esc(view.column || _('not on an archive'))],
            [_('Location'), esc(info.location)],
            [_('Moving to'), esc(info.moving_to || '')],
            [_('Label'), esc(info.label || _('(none)'))],
            [_('Size'), fsize(flags.size || 0)],
            [_('Last upload to a peer'), since(flags.since_upload)],
            [_('Last download'), since(flags.since_download)],
            [_('Why'), info.note ? esc(info.note) : ''],
        ]);

        var target = info.target;
        html += section(_('Target'), target
            ? [
                  [_('Name'), esc(target.name)],
                  [_('Mount'), esc(target.path)],
                  [
                      _('Status'),
                      target.online
                          ? _('Online')
                          : '<span style="' + orange + '">' + _('Offline') + '</span>',
                  ],
                  [_('rc URL'), esc(target.rc_url || _('none'))],
              ]
            : [[_('Target'), _('none: its files are local and its label does not archive')]]);

        var rule = info.rule;
        html += section(_('Its label\'s rule'), rule
            ? [
                  [_('Archive'), rule.archive ? yes : no],
                  [_('To target'), esc(rule.target || _('none chosen'))],
                  [
                      _('After idle for'),
                      rule.after_hours > 0 ? ftime(rule.after_hours * 3600) : _('as soon as it finishes'),
                  ],
                  [_('Bring archived torrents back'), rule.restore_local ? yes : no],
              ]
            : [[_('Label'), _('this torrent has no label, or its label is not known')]]);

        html += section(_('Rule state'), [
            [_('Paused for its upload'), flags.held_for_upload ? yes : no],
            [_('Paused while the target is offline'), flags.held_while_offline ? yes : no],
            [_('Resumed by hand during the upload'), flags.resumed_by_hand ? yes : no],
            [_('Paused'), flags.paused ? yes : no],
            [_('Being moved'), flags.moving ? yes : no],
            [_('Finished'), flags.finished ? yes : no],
        ]);

        var rclone = info.rclone || {};
        if (rclone.rc === 'none') {
            html += section(_('rclone'), [
                [_('rc'), _('no rc URL for this target: nothing to ask rclone')],
            ]);
        } else if (rclone.rc === 'unreachable') {
            html += section(_('rclone'), [
                [_('rc'), '<span style="' + orange + '">' + _('not answering') + '</span>'],
            ]);
        } else {
            var queue = (rclone.queue || []).map(function (item) {
                var what = item.uploading
                    ? _('uploading now')
                    : item.failed
                      ? '<span style="' + orange + '">' +
                        String.format(_('failed {0} time(s), next try in {1}'), item.tries, ftime(item.next_try_in)) +
                        '</span>'
                      : String.format(_('waiting, starts in {0}'), ftime(item.next_try_in));
                return [esc(item.name), what];
            });
            var transfers = (rclone.transfers || []).map(function (entry) {
                return [
                    esc(entry.name),
                    String.format(
                        _('{0}% — {1} of {2} at {3}, {4} left'),
                        entry.percentage || 0,
                        fsize(entry.bytes || 0),
                        fsize(entry.size || 0),
                        fspeed(entry.speed || 0),
                        ftime(entry.eta || 0)
                    ),
                ];
            });
            html += section(_('rclone'), [
                [_('rc'), _('answering')],
                [_('Path on the remote'), esc(rclone.relative || '')],
                [_('Upload queue'), queue.length ? '' : _('empty: nothing left to upload')],
            ]);
            if (queue.length) html += section(_('Queued files'), queue);
            if (transfers.length) html += section(_('Sending now'), transfers);
        }
        return html;
    },
});

/** The one window, built when the menu first asks for it. */
Deluge.ArchiveInfoWindow.open = function (torrentId) {
    if (!deluge.archiveInfoWindow) {
        deluge.archiveInfoWindow = new Deluge.ArchiveInfoWindow();
    }
    deluge.archiveInfoWindow.showFor(torrentId);
};
