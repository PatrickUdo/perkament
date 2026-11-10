//! Vertalingen van alle teksten in de interface en van foutmeldingen.
//!
//! Elke tekst heeft een sleutel en een vertaling per taal. De actieve taal is
//! een globale instelling, zodat ook de bibliotheeklaag (foutmeldingen) hem
//! kan gebruiken zonder dat de taal overal doorgegeven hoeft te worden.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Nl,
    En,
    De,
    Fr,
    Es,
}

impl Lang {
    pub const ALL: [Lang; 5] = [Lang::Nl, Lang::En, Lang::De, Lang::Fr, Lang::Es];

    /// Naam van de taal in die taal zelf.
    pub fn name(self) -> &'static str {
        match self {
            Lang::Nl => "Nederlands",
            Lang::En => "English",
            Lang::De => "Deutsch",
            Lang::Fr => "Français",
            Lang::Es => "Español",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::Nl => "nl",
            Lang::En => "en",
            Lang::De => "de",
            Lang::Fr => "fr",
            Lang::Es => "es",
        }
    }

    /// Herkent "nl", "nl_NL.UTF-8", "de-DE" enzovoort.
    pub fn from_code(code: &str) -> Option<Lang> {
        let prefix: String = code.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        Lang::ALL.into_iter().find(|l| l.code().eq_ignore_ascii_case(&prefix))
    }

    fn index(self) -> usize {
        self as usize
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(Lang::En as u8);

pub fn set(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

/// De taal van het besturingssysteem, of Engels als die niet wordt ondersteund.
pub fn system_language() -> Lang {
    sys_locale::get_locale().as_deref().and_then(Lang::from_code).unwrap_or(Lang::En)
}

pub fn current() -> Lang {
    Lang::ALL[CURRENT.load(Ordering::Relaxed) as usize % Lang::ALL.len()]
}

fn table() -> &'static HashMap<&'static str, [&'static str; 5]> {
    static MAP: OnceLock<HashMap<&'static str, [&'static str; 5]>> = OnceLock::new();
    MAP.get_or_init(|| STRINGS.iter().copied().collect())
}

/// Vertaling van `key` in de actieve taal. Een onbekende sleutel geeft de sleutel zelf terug.
pub fn t(key: &'static str) -> &'static str {
    t_in(current(), key)
}

pub fn t_in(lang: Lang, key: &'static str) -> &'static str {
    table().get(key).map_or(key, |row| row[lang.index()])
}

/// Als [`t`], maar vervangt `{naam}` door de meegegeven waarden.
pub fn tf(key: &'static str, values: &[(&str, &str)]) -> String {
    let mut text = t(key).to_string();
    for (name, value) in values {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

pub fn keys() -> impl Iterator<Item = &'static str> {
    STRINGS.iter().map(|(key, _)| *key)
}

/// Kolommen: Nederlands, Engels, Duits, Frans, Spaans.
#[rustfmt::skip]
static STRINGS: &[(&str, [&str; 5])] = &[
    // Menu's
    ("menu.file", ["Bestand", "File", "Datei", "Fichier", "Archivo"]),
    ("menu.open", ["Openen…", "Open…", "Öffnen…", "Ouvrir…", "Abrir…"]),
    ("menu.save", ["Opslaan", "Save", "Speichern", "Enregistrer", "Guardar"]),
    ("menu.save_as", ["Opslaan als…", "Save As…", "Speichern unter…", "Enregistrer sous…", "Guardar como…"]),
    ("menu.export_png", ["Huidige pagina exporteren als PNG…", "Export Current Page as PNG…", "Aktuelle Seite als PNG exportieren…", "Exporter la page actuelle en PNG…", "Exportar la página actual como PNG…"]),
    ("menu.extract", ["Pagina's extraheren naar nieuwe PDF…", "Extract Pages to New PDF…", "Seiten in neue PDF extrahieren…", "Extraire des pages vers un nouveau PDF…", "Extraer páginas a un PDF nuevo…"]),
    ("menu.close", ["Sluiten", "Close", "Schließen", "Fermer", "Cerrar"]),
    ("menu.quit", ["Afsluiten", "Quit", "Beenden", "Quitter", "Salir"]),
    ("menu.edit", ["Bewerken", "Edit", "Bearbeiten", "Édition", "Editar"]),
    ("menu.undo", ["Ongedaan maken", "Undo", "Rückgängig", "Annuler", "Deshacer"]),
    ("menu.redo", ["Opnieuw", "Redo", "Wiederholen", "Rétablir", "Rehacer"]),
    ("menu.pages", ["Pagina's", "Pages", "Seiten", "Pages", "Páginas"]),
    ("menu.rotate_left", ["Linksom draaien", "Rotate Left", "Nach links drehen", "Pivoter à gauche", "Girar a la izquierda"]),
    ("menu.rotate_right", ["Rechtsom draaien", "Rotate Right", "Nach rechts drehen", "Pivoter à droite", "Girar a la derecha"]),
    ("menu.move_forward", ["Naar voren verplaatsen", "Move Earlier", "Nach vorne verschieben", "Déplacer vers le début", "Mover hacia delante"]),
    ("menu.move_back", ["Naar achteren verplaatsen", "Move Later", "Nach hinten verschieben", "Déplacer vers la fin", "Mover hacia atrás"]),
    ("menu.insert_blank", ["Lege pagina invoegen na deze", "Insert Blank Page After This One", "Leere Seite nach dieser einfügen", "Insérer une page vierge après celle-ci", "Insertar página en blanco después de esta"]),
    ("menu.insert_pdf", ["PDF invoegen na deze pagina…", "Insert PDF After This Page…", "PDF nach dieser Seite einfügen…", "Insérer un PDF après cette page…", "Insertar PDF después de esta página…"]),
    ("menu.append_pdf", ["PDF toevoegen aan het einde…", "Append PDF at the End…", "PDF am Ende anfügen…", "Ajouter un PDF à la fin…", "Añadir PDF al final…"]),
    ("menu.delete_page", ["Pagina verwijderen", "Delete Page", "Seite löschen", "Supprimer la page", "Eliminar página"]),
    ("menu.document", ["Document", "Document", "Dokument", "Document", "Documento"]),
    ("menu.properties", ["Eigenschappen…", "Properties…", "Eigenschaften…", "Propriétés…", "Propiedades…"]),
    ("menu.watermark", ["Watermerk toevoegen…", "Add Watermark…", "Wasserzeichen hinzufügen…", "Ajouter un filigrane…", "Añadir marca de agua…"]),
    ("menu.page_numbers", ["Paginanummers toevoegen…", "Add Page Numbers…", "Seitenzahlen hinzufügen…", "Ajouter des numéros de page…", "Añadir números de página…"]),
    ("menu.view", ["Beeld", "View", "Ansicht", "Affichage", "Ver"]),
    ("menu.zoom_in", ["Inzoomen", "Zoom In", "Vergrößern", "Zoom avant", "Acercar"]),
    ("menu.zoom_out", ["Uitzoomen", "Zoom Out", "Verkleinern", "Zoom arrière", "Alejar"]),
    ("menu.fit_width", ["Paginabreedte", "Fit Width", "Seitenbreite", "Largeur de page", "Ancho de página"]),
    ("menu.thumbnails", ["Miniaturen", "Thumbnails", "Miniaturen", "Vignettes", "Miniaturas"]),
    ("menu.search_panel", ["Zoekpaneel", "Search Panel", "Suchleiste", "Panneau de recherche", "Panel de búsqueda"]),
    ("menu.comments", ["Opmerkingen", "Comments", "Kommentare", "Commentaires", "Comentarios"]),
    ("menu.language", ["Taal", "Language", "Sprache", "Langue", "Idioma"]),
    ("menu.help", ["Help", "Help", "Hilfe", "Aide", "Ayuda"]),
    ("menu.about", ["Over Perkament", "About Perkament", "Über Perkament", "À propos de Perkament", "Acerca de Perkament"]),

    // Werkbalk
    ("tb.open", ["Openen", "Open", "Öffnen", "Ouvrir", "Abrir"]),
    ("tb.save", ["Opslaan", "Save", "Speichern", "Enregistrer", "Guardar"]),
    ("tool.hand", ["Hand", "Hand", "Hand", "Main", "Mano"]),
    ("tool.hand.tip", ["Slepen om te scrollen", "Drag to scroll", "Ziehen zum Scrollen", "Faire glisser pour défiler", "Arrastra para desplazarte"]),
    ("tool.edit", ["Bewerken", "Edit", "Bearbeiten", "Modifier", "Editar"]),
    ("tool.edit.tip", ["Klik op een tekstregel om de tekst aan te passen", "Click a line of text to change it", "Auf eine Textzeile klicken, um sie zu ändern", "Cliquez sur une ligne de texte pour la modifier", "Haz clic en una línea de texto para cambiarla"]),
    ("tool.highlight", ["Markeren", "Highlight", "Markieren", "Surligner", "Resaltar"]),
    ("tool.highlight.tip", ["Sleep een gebied om te markeren", "Drag over an area to highlight it", "Bereich zum Markieren aufziehen", "Faites glisser sur une zone pour la surligner", "Arrastra sobre una zona para resaltarla"]),
    ("tool.rect", ["Kader", "Box", "Rahmen", "Cadre", "Recuadro"]),
    ("tool.rect.tip", ["Sleep om een kader te tekenen", "Drag to draw a box", "Ziehen, um einen Rahmen zu zeichnen", "Faites glisser pour tracer un cadre", "Arrastra para dibujar un recuadro"]),
    ("tool.note", ["Notitie", "Note", "Notiz", "Note", "Nota"]),
    ("tool.note.tip", ["Klik om een notitie te plaatsen", "Click to place a note", "Klicken, um eine Notiz zu setzen", "Cliquez pour placer une note", "Haz clic para colocar una nota"]),
    ("tool.text", ["Tekst", "Text", "Text", "Texte", "Texto"]),
    ("tool.text.tip", ["Klik of sleep om een tekstvak te plaatsen", "Click or drag to place a text box", "Klicken oder ziehen, um ein Textfeld zu setzen", "Cliquez ou faites glisser pour placer une zone de texte", "Haz clic o arrastra para colocar un cuadro de texto"]),
    ("tool.pen", ["Pen", "Pen", "Stift", "Stylo", "Lápiz"]),
    ("tool.pen.tip", ["Teken uit de vrije hand", "Draw freehand", "Freihand zeichnen", "Dessiner à main levée", "Dibuja a mano alzada"]),
    ("tb.color.tip", ["Kleur van opmerkingen", "Comment colour", "Farbe der Kommentare", "Couleur des commentaires", "Color de los comentarios"]),
    ("tb.width", ["Breedte", "Width", "Breite", "Largeur", "Ancho"]),
    ("tb.width.tip", ["Passend op paginabreedte", "Fit to page width", "An Seitenbreite anpassen", "Ajuster à la largeur de la page", "Ajustar al ancho de página"]),
    ("tb.rotate_left.tip", ["Pagina linksom draaien", "Rotate page left", "Seite nach links drehen", "Pivoter la page à gauche", "Girar la página a la izquierda"]),
    ("tb.rotate_right.tip", ["Pagina rechtsom draaien", "Rotate page right", "Seite nach rechts drehen", "Pivoter la page à droite", "Girar la página a la derecha"]),
    ("tb.search", ["Zoek", "Search", "Suchen", "Chercher", "Buscar"]),
    ("tb.search.hint", ["Zoeken in document…", "Search in document…", "Im Dokument suchen…", "Rechercher dans le document…", "Buscar en el documento…"]),

    // Contextmenu van miniaturen
    ("ctx.forward", ["Naar voren", "Move earlier", "Nach vorne", "Vers le début", "Hacia delante"]),
    ("ctx.back", ["Naar achteren", "Move later", "Nach hinten", "Vers la fin", "Hacia atrás"]),
    ("ctx.blank_after", ["Lege pagina hierna", "Blank page after this", "Leere Seite danach", "Page vierge après", "Página en blanco después"]),
    ("ctx.pdf_after", ["PDF hierna invoegen…", "Insert PDF after this…", "PDF danach einfügen…", "Insérer un PDF après…", "Insertar PDF después…"]),
    ("ctx.delete", ["Verwijderen", "Delete", "Löschen", "Supprimer", "Eliminar"]),

    // Zijpaneel
    ("side.search", ["Zoeken", "Search", "Suche", "Recherche", "Búsqueda"]),
    ("side.search.prompt", ["Typ een zoekterm in de werkbalk en druk op Enter.", "Type a search term in the toolbar and press Enter.", "Suchbegriff in die Symbolleiste eingeben und Enter drücken.", "Saisissez un terme dans la barre d'outils et appuyez sur Entrée.", "Escribe un término en la barra de herramientas y pulsa Intro."]),
    ("side.no_results", ["Geen resultaten.", "No results.", "Keine Treffer.", "Aucun résultat.", "Sin resultados."]),
    ("side.results", ["{n} resultaten", "{n} results", "{n} Treffer", "{n} résultats", "{n} resultados"]),
    ("side.no_comments", ["Nog geen opmerkingen. Kies een gereedschap in de werkbalk.", "No comments yet. Pick a tool in the toolbar.", "Noch keine Kommentare. Wähle ein Werkzeug in der Symbolleiste.", "Aucun commentaire pour l'instant. Choisissez un outil dans la barre d'outils.", "Aún no hay comentarios. Elige una herramienta en la barra."]),
    ("side.delete", ["Verwijder", "Delete", "Löschen", "Supprimer", "Eliminar"]),
    ("page.abbr", ["p. {n}", "p. {n}", "S. {n}", "p. {n}", "pág. {n}"]),

    // Startscherm en paginaweergave
    ("welcome.tagline", ["PDF's bekijken, annoteren en herschikken, geschreven in Rust.", "View, annotate and rearrange PDFs, written in Rust.", "PDFs ansehen, kommentieren und neu ordnen, geschrieben in Rust.", "Consultez, annotez et réorganisez vos PDF, écrit en Rust.", "Consulta, anota y reorganiza tus PDF, escrito en Rust."]),
    ("welcome.open", ["PDF openen…", "Open PDF…", "PDF öffnen…", "Ouvrir un PDF…", "Abrir PDF…"]),
    ("welcome.drop", ["of sleep een PDF-bestand naar dit venster", "or drag a PDF file onto this window", "oder ziehe eine PDF-Datei in dieses Fenster", "ou faites glisser un fichier PDF dans cette fenêtre", "o arrastra un archivo PDF a esta ventana"]),
    ("view.rendering", ["Bezig met renderen…", "Rendering…", "Wird gerendert…", "Rendu en cours…", "Renderizando…"]),
    ("view.failed", ["Deze pagina kon niet worden weergegeven.", "This page could not be displayed.", "Diese Seite konnte nicht angezeigt werden.", "Cette page n'a pas pu être affichée.", "No se pudo mostrar esta página."]),
    ("untitled", ["Naamloos.pdf", "Untitled.pdf", "Unbenannt.pdf", "Sans titre.pdf", "Sin título.pdf"]),

    // Knoppen
    ("btn.apply", ["Toepassen", "Apply", "Anwenden", "Appliquer", "Aplicar"]),
    ("btn.cancel", ["Annuleren", "Cancel", "Abbrechen", "Annuler", "Cancelar"]),
    ("btn.add", ["Toevoegen", "Add", "Hinzufügen", "Ajouter", "Añadir"]),
    ("btn.ok", ["OK", "OK", "OK", "OK", "Aceptar"]),
    ("btn.close", ["Sluiten", "Close", "Schließen", "Fermer", "Cerrar"]),
    ("btn.save", ["Opslaan", "Save", "Speichern", "Enregistrer", "Guardar"]),
    ("btn.dont_save", ["Niet opslaan", "Don't Save", "Nicht speichern", "Ne pas enregistrer", "No guardar"]),
    ("btn.place", ["Plaatsen", "Place", "Einfügen", "Placer", "Colocar"]),
    ("btn.save_as", ["Opslaan als…", "Save As…", "Speichern unter…", "Enregistrer sous…", "Guardar como…"]),

    // Dialogen
    ("dlg.properties", ["Documenteigenschappen", "Document Properties", "Dokumenteigenschaften", "Propriétés du document", "Propiedades del documento"]),
    ("meta.title", ["Titel", "Title", "Titel", "Titre", "Título"]),
    ("meta.author", ["Auteur", "Author", "Autor", "Auteur", "Autor"]),
    ("meta.subject", ["Onderwerp", "Subject", "Thema", "Sujet", "Asunto"]),
    ("meta.keywords", ["Trefwoorden", "Keywords", "Stichwörter", "Mots-clés", "Palabras clave"]),
    ("meta.creator", ["Gemaakt met", "Created with", "Erstellt mit", "Créé avec", "Creado con"]),
    ("meta.pages", ["Pagina's", "Pages", "Seiten", "Pages", "Páginas"]),
    ("dlg.watermark", ["Watermerk toevoegen", "Add Watermark", "Wasserzeichen hinzufügen", "Ajouter un filigrane", "Añadir marca de agua"]),
    ("wm.text", ["Tekst", "Text", "Text", "Texte", "Texto"]),
    ("wm.size", ["Grootte", "Size", "Größe", "Taille", "Tamaño"]),
    ("wm.opacity", ["Dekking", "Opacity", "Deckkraft", "Opacité", "Opacidad"]),
    ("wm.color", ["Kleur", "Colour", "Farbe", "Couleur", "Color"]),
    ("wm.direction", ["Richting", "Direction", "Ausrichtung", "Orientation", "Orientación"]),
    ("wm.diagonal", ["Diagonaal", "Diagonal", "Diagonal", "En diagonale", "Diagonal"]),
    ("wm.all_pages", ["Het watermerk komt op alle pagina's.", "The watermark is added to every page.", "Das Wasserzeichen kommt auf alle Seiten.", "Le filigrane est ajouté sur toutes les pages.", "La marca de agua se añade a todas las páginas."]),
    ("wm.default", ["CONCEPT", "DRAFT", "ENTWURF", "BROUILLON", "BORRADOR"]),
    ("dlg.page_numbers", ["Paginanummers toevoegen", "Add Page Numbers", "Seitenzahlen hinzufügen", "Ajouter des numéros de page", "Añadir números de página"]),
    ("pn.format", ["Opmaak ({n} = paginanummer, {N} = totaal):", "Format ({n} = page number, {N} = total):", "Format ({n} = Seitenzahl, {N} = Gesamtzahl):", "Format ({n} = numéro de page, {N} = total) :", "Formato ({n} = número de página, {N} = total):"]),
    ("pn.default", ["Pagina {n} van {N}", "Page {n} of {N}", "Seite {n} von {N}", "Page {n} sur {N}", "Página {n} de {N}"]),
    ("dlg.extract", ["Pagina's extraheren", "Extract Pages", "Seiten extrahieren", "Extraire des pages", "Extraer páginas"]),
    ("ex.from", ["Van pagina", "From page", "Von Seite", "De la page", "Desde la página"]),
    ("ex.to", ["t/m", "to", "bis", "à", "hasta"]),
    ("dlg.note", ["Notitie", "Note", "Notiz", "Note", "Nota"]),
    ("dlg.textbox", ["Tekstvak", "Text Box", "Textfeld", "Zone de texte", "Cuadro de texto"]),
    ("dlg.text.hint", ["Typ je tekst…", "Type your text…", "Text eingeben…", "Saisissez votre texte…", "Escribe tu texto…"]),
    ("dlg.edit_text", ["Tekst bewerken", "Edit Text", "Text bearbeiten", "Modifier le texte", "Editar texto"]),
    ("edit.hint", ["De regel behoudt positie en lettertype; de rest van de pagina schuift niet mee.", "The line keeps its position and font; the rest of the page does not reflow.", "Die Zeile behält Position und Schrift; der Rest der Seite rückt nicht nach.", "La ligne conserve sa position et sa police ; le reste de la page n'est pas réagencé.", "La línea conserva su posición y su fuente; el resto de la página no se recoloca."]),
    ("dlg.unsaved", ["Niet-opgeslagen wijzigingen", "Unsaved Changes", "Ungespeicherte Änderungen", "Modifications non enregistrées", "Cambios sin guardar"]),
    ("unsaved.body", ["Dit document heeft wijzigingen die nog niet zijn opgeslagen.", "This document has changes that have not been saved yet.", "Dieses Dokument enthält Änderungen, die noch nicht gespeichert sind.", "Ce document contient des modifications qui n'ont pas encore été enregistrées.", "Este documento tiene cambios que aún no se han guardado."]),
    ("dlg.error", ["Er ging iets mis", "Something Went Wrong", "Etwas ist schiefgelaufen", "Un problème est survenu", "Algo salió mal"]),
    ("about.version", ["Versie {v}", "Version {v}", "Version {v}", "Version {v}", "Versión {v}"]),
    ("about.body", ["Een PDF-editor in pure Rust: hayro (weergave), lopdf (bewerken), egui (interface).", "A PDF editor in pure Rust: hayro (rendering), lopdf (editing), egui (interface).", "Ein PDF-Editor in reinem Rust: hayro (Darstellung), lopdf (Bearbeitung), egui (Oberfläche).", "Un éditeur de PDF en Rust pur : hayro (rendu), lopdf (édition), egui (interface).", "Un editor de PDF en Rust puro: hayro (renderizado), lopdf (edición), egui (interfaz)."]),

    // Statusbalk
    ("st.start", ["Open een PDF om te beginnen.", "Open a PDF to get started.", "Öffne eine PDF, um zu beginnen.", "Ouvrez un PDF pour commencer.", "Abre un PDF para empezar."]),
    ("st.opened", ["{path} geopend.", "Opened {path}.", "{path} geöffnet.", "{path} ouvert.", "{path} abierto."]),
    ("st.saved", ["Opgeslagen als {path}.", "Saved as {path}.", "Gespeichert unter {path}.", "Enregistré sous {path}.", "Guardado como {path}."]),
    ("st.closed", ["Document gesloten.", "Document closed.", "Dokument geschlossen.", "Document fermé.", "Documento cerrado."]),
    ("st.undone", ["Ongedaan gemaakt.", "Undone.", "Rückgängig gemacht.", "Action annulée.", "Deshecho."]),
    ("st.redone", ["Opnieuw uitgevoerd.", "Redone.", "Wiederholt.", "Action rétablie.", "Rehecho."]),
    ("st.rotated", ["Pagina gedraaid.", "Page rotated.", "Seite gedreht.", "Page pivotée.", "Página girada."]),
    ("st.page_deleted", ["Pagina verwijderd.", "Page deleted.", "Seite gelöscht.", "Page supprimée.", "Página eliminada."]),
    ("st.page_moved", ["Pagina verplaatst.", "Page moved.", "Seite verschoben.", "Page déplacée.", "Página movida."]),
    ("st.blank_inserted", ["Lege pagina ingevoegd.", "Blank page inserted.", "Leere Seite eingefügt.", "Page vierge insérée.", "Página en blanco insertada."]),
    ("st.pdf_inserted", ["PDF ingevoegd.", "PDF inserted.", "PDF eingefügt.", "PDF inséré.", "PDF insertado."]),
    ("st.extracted", ["Pagina {from} t/m {to} opgeslagen als {path}.", "Pages {from} to {to} saved as {path}.", "Seiten {from} bis {to} gespeichert unter {path}.", "Pages {from} à {to} enregistrées sous {path}.", "Páginas {from} a {to} guardadas como {path}."]),
    ("st.exported", ["Pagina geëxporteerd naar {path}.", "Page exported to {path}.", "Seite exportiert nach {path}.", "Page exportée vers {path}.", "Página exportada a {path}."]),
    ("st.meta_updated", ["Documenteigenschappen bijgewerkt.", "Document properties updated.", "Dokumenteigenschaften aktualisiert.", "Propriétés du document mises à jour.", "Propiedades del documento actualizadas."]),
    ("st.watermark_added", ["Watermerk toegevoegd.", "Watermark added.", "Wasserzeichen hinzugefügt.", "Filigrane ajouté.", "Marca de agua añadida."]),
    ("st.numbers_added", ["Paginanummers toegevoegd.", "Page numbers added.", "Seitenzahlen hinzugefügt.", "Numéros de page ajoutés.", "Números de página añadidos."]),
    ("st.annot_added", ["Opmerking toegevoegd.", "Comment added.", "Kommentar hinzugefügt.", "Commentaire ajouté.", "Comentario añadido."]),
    ("st.annot_deleted", ["Opmerking verwijderd.", "Comment deleted.", "Kommentar gelöscht.", "Commentaire supprimé.", "Comentario eliminado."]),
    ("st.text_changed", ["Tekst aangepast.", "Text changed.", "Text geändert.", "Texte modifié.", "Texto modificado."]),
    ("st.text_substitute", ["Tekst aangepast in een standaardlettertype: het oorspronkelijke lettertype mist tekens.", "Text changed using a standard font: the original font lacks some characters.", "Text in einer Standardschrift geändert: der ursprünglichen Schrift fehlen Zeichen.", "Texte modifié avec une police standard : la police d'origine ne contient pas tous les caractères.", "Texto modificado con una fuente estándar: a la fuente original le faltan caracteres."]),
    ("st.results", ["{n} resultaten voor \"{q}\".", "{n} results for \"{q}\".", "{n} Treffer für \"{q}\".", "{n} résultats pour \"{q}\".", "{n} resultados para \"{q}\"."]),
    ("st.page_of", ["Pagina {n} van {N}", "Page {n} of {N}", "Seite {n} von {N}", "Page {n} sur {N}", "Página {n} de {N}"]),

    // Foutmeldingen in de interface
    ("err.read_file", ["Kan bestand niet lezen: {e}", "Cannot read file: {e}", "Datei kann nicht gelesen werden: {e}", "Impossible de lire le fichier : {e}", "No se puede leer el archivo: {e}"]),
    ("err.cannot_read", ["Kan {path} niet lezen: {e}", "Cannot read {path}: {e}", "{path} kann nicht gelesen werden: {e}", "Impossible de lire {path} : {e}", "No se puede leer {path}: {e}"]),
    ("err.save", ["Opslaan mislukt: {e}", "Saving failed: {e}", "Speichern fehlgeschlagen: {e}", "Échec de l'enregistrement : {e}", "No se pudo guardar: {e}"]),
    ("err.extract", ["Extraheren mislukt: {e}", "Extracting failed: {e}", "Extrahieren fehlgeschlagen: {e}", "Échec de l'extraction : {e}", "No se pudo extraer: {e}"]),
    ("err.export", ["Exporteren mislukt: {e}", "Exporting failed: {e}", "Exportieren fehlgeschlagen: {e}", "Échec de l'exportation : {e}", "No se pudo exportar: {e}"]),
    ("err.display", ["Kan document niet weergeven: {e}", "Cannot display document: {e}", "Dokument kann nicht angezeigt werden: {e}", "Impossible d'afficher le document : {e}", "No se puede mostrar el documento: {e}"]),

    // Foutmeldingen uit de bewerkingslaag
    ("err.pdf_read", ["kan PDF niet lezen", "cannot read PDF", "PDF kann nicht gelesen werden", "impossible de lire le PDF", "no se puede leer el PDF"]),
    ("err.password", ["deze PDF is met een wachtwoord beveiligd", "this PDF is password-protected", "diese PDF ist passwortgeschützt", "ce PDF est protégé par un mot de passe", "este PDF está protegido con contraseña"]),
    ("err.serialize", ["kan PDF niet wegschrijven", "cannot write PDF", "PDF kann nicht geschrieben werden", "impossible d'écrire le PDF", "no se puede escribir el PDF"]),
    ("err.no_page", ["pagina {n} bestaat niet", "page {n} does not exist", "Seite {n} existiert nicht", "la page {n} n'existe pas", "la página {n} no existe"]),
    ("err.no_pages", ["de PDF bevat geen pagina's", "the PDF contains no pages", "die PDF enthält keine Seiten", "le PDF ne contient aucune page", "el PDF no contiene páginas"]),
    ("err.keep_one", ["een document moet minstens één pagina houden", "a document must keep at least one page", "ein Dokument muss mindestens eine Seite behalten", "un document doit conserver au moins une page", "un documento debe conservar al menos una página"]),
    ("err.delete_all", ["je kunt niet alle pagina's verwijderen", "you cannot delete every page", "es können nicht alle Seiten gelöscht werden", "impossible de supprimer toutes les pages", "no se pueden eliminar todas las páginas"]),
    ("err.out_of_range", ["pagina buiten bereik", "page out of range", "Seite außerhalb des Bereichs", "page hors limites", "página fuera de rango"]),
    ("err.ink_points", ["te weinig punten voor een pentekening", "too few points for a pen drawing", "zu wenige Punkte für eine Stiftzeichnung", "trop peu de points pour un tracé", "muy pocos puntos para un trazo"]),
    ("err.too_small", ["de selectie is te klein", "the selection is too small", "die Auswahl ist zu klein", "la sélection est trop petite", "la selección es demasiado pequeña"]),
    ("err.wm_empty", ["vul een watermerktekst in", "enter a watermark text", "gib einen Wasserzeichentext ein", "saisissez le texte du filigrane", "introduce el texto de la marca de agua"]),
    ("err.content_read", ["kan pagina-inhoud niet lezen", "cannot read the page content", "Seiteninhalt kann nicht gelesen werden", "impossible de lire le contenu de la page", "no se puede leer el contenido de la página"]),
    ("err.content_encode", ["kan pagina-inhoud niet wegschrijven", "cannot write the page content", "Seiteninhalt kann nicht geschrieben werden", "impossible d'écrire le contenu de la page", "no se puede escribir el contenido de la página"]),
    ("err.text_stale", ["de tekst op deze pagina is intussen gewijzigd; probeer het opnieuw", "the text on this page has changed in the meantime; please try again", "der Text auf dieser Seite wurde inzwischen geändert; bitte erneut versuchen", "le texte de cette page a changé entre-temps ; veuillez réessayer", "el texto de esta página ha cambiado entretanto; inténtalo de nuevo"]),
    ("err.font_missing", ["lettertype niet gevonden", "font not found", "Schrift nicht gefunden", "police introuvable", "fuente no encontrada"]),
    ("err.text_pos", ["ongeldige tekstpositie", "invalid text position", "ungültige Textposition", "position de texte invalide", "posición de texto no válida"]),
    ("err.unsupported_chars", ["deze tekens worden niet ondersteund: {chars}", "these characters are not supported: {chars}", "diese Zeichen werden nicht unterstützt: {chars}", "ces caractères ne sont pas pris en charge : {chars}", "estos caracteres no son compatibles: {chars}"]),
    ("err.page_missing", ["pagina bestaat niet", "page does not exist", "Seite existiert nicht", "la page n'existe pas", "la página no existe"]),
    ("err.usage", ["gebruik: perkament --render in.pdf uitvoermap [schaal]", "usage: perkament --render in.pdf output-dir [scale]", "Aufruf: perkament --render in.pdf ausgabeordner [skalierung]", "usage : perkament --render in.pdf dossier-sortie [échelle]", "uso: perkament --render in.pdf carpeta-salida [escala]"]),
    ("err.prefix", ["fout", "error", "Fehler", "erreur", "error"]),

    // Soorten opmerkingen
    ("annot.highlight", ["Markering", "Highlight", "Markierung", "Surlignage", "Resaltado"]),
    ("annot.square", ["Kader", "Box", "Rahmen", "Cadre", "Recuadro"]),
    ("annot.note", ["Notitie", "Note", "Notiz", "Note", "Nota"]),
    ("annot.freetext", ["Tekstvak", "Text box", "Textfeld", "Zone de texte", "Cuadro de texto"]),
    ("annot.ink", ["Pen", "Pen", "Stift", "Tracé", "Trazo"]),
    ("annot.underline", ["Onderstreping", "Underline", "Unterstreichung", "Soulignement", "Subrayado"]),
    ("annot.strikeout", ["Doorhaling", "Strikethrough", "Durchstreichung", "Barré", "Tachado"]),
    ("annot.stamp", ["Stempel", "Stamp", "Stempel", "Tampon", "Sello"]),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(text: &str) -> Vec<String> {
        let mut found: Vec<String> = text
            .split('{')
            .skip(1)
            .filter_map(|part| part.split_once('}').map(|(name, _)| name.to_string()))
            .collect();
        found.sort();
        found
    }

    #[test]
    fn table_is_complete_and_consistent() {
        assert_eq!(table().len(), STRINGS.len(), "dubbele sleutel in de vertaaltabel");
        for (key, row) in STRINGS {
            for (lang, text) in Lang::ALL.iter().zip(row) {
                assert!(!text.trim().is_empty(), "{key} mist een vertaling voor {lang:?}");
                assert!(!text.contains('\u{2014}') && !text.contains('\u{2013}'), "{key} bevat een lang streepje");
                assert_eq!(placeholders(text), placeholders(row[0]), "{key}: plaatshouders verschillen in {lang:?}");
            }
        }
    }

    #[test]
    fn language_codes() {
        assert_eq!(Lang::from_code("nl_NL.UTF-8"), Some(Lang::Nl));
        assert_eq!(Lang::from_code("de-DE"), Some(Lang::De));
        assert_eq!(Lang::from_code("ja"), None);
        assert_eq!(tf("side.results", &[("n", "3")]).contains('3'), true);
    }
}
