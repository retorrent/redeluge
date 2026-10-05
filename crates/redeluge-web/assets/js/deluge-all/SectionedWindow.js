/**
 * Deluge.SectionedWindow.js
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * A settings window laid out the way Preferences is: a list of sections on
 * the left and one section at a time on the right, so a window with many
 * groups does not become one long scroll.
 *
 * The label and tracker settings windows are built on it. A subclass calls
 * `addSection(title)` and fills in the panel it gets back; the fields stay
 * wherever the subclass keeps them, since nothing here reads or writes them.
 */
Ext.ns('Deluge');

/**
 * @class Deluge.SectionedWindow
 * @extends Ext.Window
 */
Deluge.SectionedWindow = Ext.extend(Ext.Window, {
    layout: 'border',
    width: 640,
    height: 460,
    minWidth: 480,
    minHeight: 300,
    resizable: true,
    buttonAlign: 'right',
    closeAction: 'hide',
    constrainHeader: true,
    plain: true,

    initComponent: function () {
        Deluge.SectionedWindow.superclass.initComponent.call(this);

        this.list = new Ext.list.ListView({
            store: new Ext.data.ArrayStore({ fields: ['name'] }),
            columns: [{ id: 'name', dataIndex: 'name' }],
            singleSelect: true,
            hideHeaders: true,
            autoScroll: true,
            listeners: {
                selectionchange: { fn: this.onSectionSelect, scope: this },
            },
        });
        this.add({
            region: 'west',
            items: [this.list],
            width: 150,
            margins: '0 5 0 0',
            autoScroll: true,
        });

        this.cards = this.add({
            region: 'center',
            layout: 'card',
            activeItem: 0,
            border: false,
            // Every section rendered up front: the windows fill their fields
            // in as soon as they open, whichever section is showing. Laid
            // out again when shown, and hidden by moving off screen rather
            // than `display: none`: a composite field (days and hours) sizes
            // its boxes when it is laid out, and inside a hidden card it
            // measured nothing and drew nothing.
            layoutConfig: { deferredRender: false, layoutOnCardChange: true },
            defaults: { hideMode: 'offsets' },
        });

        this.on(
            'afterrender',
            function () {
                if (!this.list.getSelectionCount()) this.list.select(0);
            },
            this
        );
    },

    /**
     * Adds a section and answers the panel to fill in.
     */
    addSection: function (title) {
        var store = this.list.getStore();
        store.add(new store.recordType({ name: title }));
        return this.cards.add({
            xtype: 'form',
            border: false,
            autoScroll: true,
            bodyStyle: 'padding: 5px',
            labelWidth: 170,
        });
    },

    onSectionSelect: function (list, selections) {
        if (!selections.length) return;
        this.cards.getLayout().setActiveItem(list.indexOf(selections[0]));
    },
});
